#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::Duration;

use ttywright::core::db::{
    DbConnection, DbEndpoint, DbGrid, DbOutcome, DbServer, Safety, classify, tables_from,
};
use ttywright::services::db::DbHost;

fn run(host: &DbHost, connection: &DbConnection, sql: &str, read_only: bool) -> DbOutcome {
    let (tx, rx) = channel();
    host.query(
        1,
        connection.clone(),
        sql.to_owned(),
        read_only,
        move |_, outcome| {
            let _ = tx.send(outcome);
        },
    );
    rx.recv_timeout(Duration::from_secs(30)).expect("answered")
}

fn done(outcome: DbOutcome) -> DbGrid {
    match outcome {
        DbOutcome::Done(grid) => grid,
        other => panic!("{other:?}"),
    }
}

fn has(program: &str) -> bool {
    Command::new("which")
        .arg(program)
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn sqlite_at(path: &Path) -> DbConnection {
    DbConnection {
        name: "t".to_owned(),
        endpoint: DbEndpoint::Sqlite {
            path: path.to_path_buf(),
        },
    }
}

#[test]
fn sqlite_reads_writes_lists_and_refuses() {
    if !has("sqlite3") {
        eprintln!("sqlite3 is not here");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    std::fs::write(&path, "").unwrap();
    let c = sqlite_at(&path);
    let host = DbHost::default();

    let grid = done(run(
        &host,
        &c,
        "CREATE TABLE users(id INTEGER, note TEXT); \
         INSERT INTO users VALUES (1, 'two' || char(10) || 'lines, \"q\"'), \
         (2, char(27) || '[31mred'), (3, NULL)",
        false,
    ));
    assert!(grid.columns.is_empty(), "{grid:?}");

    let grid = done(run(
        &host,
        &c,
        "SELECT * FROM users ORDER BY id -- end",
        true,
    ));
    assert_eq!(grid.columns, ["id", "note"]);
    assert_eq!(grid.rows[0][1], "two↵lines, \"q\"");
    assert_eq!(grid.rows[1][1], "�[31mred", "no escape reaches the screen");
    assert_eq!(grid.rows[2][1], "NULL");

    let tables = tables_from(&done(run(&host, &c, c.tables_query(), true)));
    assert_eq!(tables[0].name, "users");
    let browse = done(run(&host, &c, &c.browse_query(&tables[0]), true));
    assert_eq!(browse.rows.len(), 3);

    let DbOutcome::Failed(message) = run(&host, &c, "DELETE FROM users", true) else {
        panic!("a read-only client wrote");
    };
    assert!(
        message.contains("readonly") || message.contains("read-only"),
        "{message}"
    );
    assert_eq!(
        done(run(&host, &c, "SELECT * FROM users", true)).rows.len(),
        3
    );

    let DbOutcome::Failed(message) = run(&host, &c, "SELEC 1", true) else {
        panic!("not an error");
    };
    assert!(message.to_lowercase().contains("error"), "{message}");
}

#[test]
fn sqlite_safe_mode_closes_what_could_reach_the_machine() {
    if !has("sqlite3") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    std::fs::write(&path, "").unwrap();
    let c = sqlite_at(&path);
    let host = DbHost::default();
    let marker = dir.path().join("marker");
    let attach = format!("ATTACH '{}' AS x", dir.path().join("other.db").display());
    let shell = format!(".shell touch {}", marker.display());
    let write_file = format!("SELECT writefile('{}', 'x')", marker.display());
    for statement in [attach.as_str(), shell.as_str(), write_file.as_str()] {
        let _ = run(&host, &c, statement, false);
    }
    assert!(!marker.exists(), "safe mode let a file be written");
    assert!(!dir.path().join("other.db").exists());
}

#[test]
fn a_missing_file_is_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("typo.db");
    let outcome = run(
        &DbHost::default(),
        &sqlite_at(&path),
        "CREATE TABLE t(a)",
        false,
    );
    assert!(matches!(outcome, DbOutcome::Failed(_)), "{outcome:?}");
    assert!(!path.exists());
}

#[test]
fn stop_ends_a_long_query() {
    if !has("sqlite3") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    std::fs::write(&path, "").unwrap();
    let host = DbHost::default();
    let endless = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n \
                   WHERE i < 100000000000) SELECT count(*) FROM n";
    let (tx, rx) = channel();
    host.query(
        7,
        sqlite_at(&path),
        endless.to_owned(),
        true,
        move |_, o| {
            let _ = tx.send(o);
        },
    );
    std::thread::sleep(Duration::from_millis(200));
    host.cancel(7);
    let outcome = rx.recv_timeout(Duration::from_secs(10)).expect("stopped");
    assert_eq!(outcome, DbOutcome::Cancelled);
}

#[test]
fn a_password_variable_that_is_not_set_is_named_and_nothing_starts() {
    let c = DbConnection {
        name: "p".to_owned(),
        endpoint: DbEndpoint::Postgres {
            server: DbServer::default(),
            password_env: Some("TTYWRIGHT_TEST_NO_SUCH_VARIABLE".to_owned()),
        },
    };
    let DbOutcome::Failed(message) = run(&DbHost::default(), &c, "SELECT 1", true) else {
        panic!("should fail");
    };
    assert!(
        message.contains("TTYWRIGHT_TEST_NO_SUCH_VARIABLE") || message.contains("not installed"),
        "{message}"
    );
}

struct Server {
    child: std::process::Child,
    _dir: tempfile::TempDir,
    socket: PathBuf,
    defaults: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_mariadb() -> Server {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let data = dir.path().join("data");
    let socket = dir.path().join("s.sock");
    let status = Command::new("mariadb-install-db")
        .arg(format!("--datadir={}", data.display()))
        .args(["--auth-root-authentication-method=normal", "--skip-test-db"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("mariadb-install-db");
    assert!(status.success());
    let child = Command::new("mariadbd")
        .arg("--no-defaults")
        .arg(format!("--datadir={}", data.display()))
        .arg(format!("--socket={}", socket.display()))
        .args(["--skip-networking", "--skip-grant-tables"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("mariadbd");
    let defaults = dir.path().join("client.cnf");
    std::fs::write(
        &defaults,
        format!("[client]\nuser=root\nsocket={}\n", socket.display()),
    )
    .unwrap();
    let server = Server {
        child,
        _dir: dir,
        socket,
        defaults,
    };
    for _ in 0..100 {
        if server.socket.exists() {
            return server;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("mariadbd did not start");
}

#[test]
#[ignore = "needs mariadbd and mariadb-install-db"]
fn real_mariadb_through_the_mysql_client() {
    let server = start_mariadb();
    let base = DbConnection {
        name: "m".to_owned(),
        endpoint: DbEndpoint::Mysql {
            server: DbServer::default(),
            defaults_file: Some(server.defaults.clone()),
        },
    };
    let host = DbHost::default();
    done(run(&host, &base, "CREATE DATABASE demo", false));
    let c = DbConnection {
        name: "m".to_owned(),
        endpoint: DbEndpoint::Mysql {
            server: DbServer {
                database: Some("demo".to_owned()),
                ..DbServer::default()
            },
            defaults_file: Some(server.defaults.clone()),
        },
    };
    done(run(
        &host,
        &c,
        "CREATE TABLE users(id INT, note TEXT); \
         INSERT INTO users VALUES (1, 'a\\tb\\nc'), (2, NULL)",
        false,
    ));
    let grid = done(run(
        &host,
        &c,
        "SELECT * FROM users ORDER BY id -- end",
        true,
    ));
    assert_eq!(grid.columns, ["id", "note"]);
    assert_eq!(grid.rows[0][1], "a b↵c");
    assert_eq!(grid.rows[1][1], "NULL");
    let tables = tables_from(&done(run(&host, &c, c.tables_query(), true)));
    assert_eq!(tables[0].name, "users");
    assert_eq!(
        done(run(&host, &c, &c.browse_query(&tables[0]), true))
            .rows
            .len(),
        2
    );

    let marker = server.socket.with_file_name("marker");
    let bang = format!("\\! touch {}", marker.display());
    let word = format!("system touch {}", marker.display());
    assert_eq!(
        classify(ttywright::core::db::DbKind::Mysql, &bang),
        Safety::Write
    );
    for statement in [bang.as_str(), word.as_str()] {
        let _ = run(&host, &c, statement, false);
    }
    assert!(!marker.exists(), "a client command ran");

    let outcome = run(&host, &c, "DELETE FROM users", true);
    assert!(matches!(outcome, DbOutcome::Failed(_)), "{outcome:?}");
    assert_eq!(
        done(run(&host, &c, "SELECT * FROM users", true)).rows.len(),
        2
    );

    let (tx, rx) = channel();
    host.query(
        9,
        c.clone(),
        "SELECT SLEEP(60)".to_owned(),
        false,
        move |_, o| {
            let _ = tx.send(o);
        },
    );
    std::thread::sleep(Duration::from_millis(500));
    host.cancel(9);
    let outcome = rx.recv_timeout(Duration::from_secs(10)).expect("stopped");
    assert_eq!(outcome, DbOutcome::Cancelled);
}
