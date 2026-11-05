use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use ttywright::services::fs::write_atomic;
use ttywright::services::swap::SwapDir;

const WORK_DIR: &str = "CRASH_SAFETY_WORK_DIR";

fn version_text(n: u64) -> String {
    format!("{n}\n").repeat(20_000)
}

fn parse_version(text: &str) -> Option<u64> {
    let n = text.lines().next()?.parse().ok()?;
    (text == version_text(n)).then_some(n)
}

#[allow(clippy::unwrap_used)]
fn edit_until_killed(dir: &Path) {
    let target = dir.join("file.txt");
    let swaps = SwapDir::new(dir.join("swap"));
    let mut out = std::io::stdout();
    for n in 1.. {
        let text = version_text(n);
        swaps.write(&target, None, text.as_bytes()).unwrap();
        writeln!(out, "swap {n}").unwrap();
        out.flush().unwrap();
        write_atomic(&target, text.as_bytes()).unwrap();
    }
}

#[test]
fn killed_process_leaves_whole_file_and_latest_swap() {
    if let Ok(dir) = std::env::var(WORK_DIR) {
        return edit_until_killed(Path::new(&dir));
    }
    for delay_ms in [0, 3, 7, 11, 19, 31, 47, 83] {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("swap")).unwrap();
        let target = tmp.path().join("file.txt");
        std::fs::write(&target, version_text(0)).unwrap();

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "killed_process_leaves_whole_file_and_latest_swap",
                "--exact",
                "--nocapture",
            ])
            .env(WORK_DIR, tmp.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut acked = 0;
        for line in lines.by_ref() {
            if let Some(n) = line.unwrap().strip_prefix("swap ") {
                acked = n.parse().unwrap();
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(delay_ms));
        child.kill().unwrap();
        child.wait().unwrap();
        for line in lines {
            if let Some(n) = line.unwrap().strip_prefix("swap ")
                && let Ok(n) = n.parse()
            {
                acked = n;
            }
        }

        let on_disk = std::fs::read_to_string(&target).unwrap();
        assert!(
            parse_version(&on_disk).is_some(),
            "torn file after {delay_ms} ms"
        );

        let swaps = SwapDir::new(tmp.path().join("swap")).recover();
        let newest = match swaps.first() {
            Some(swap) => {
                let text = String::from_utf8(swap.contents.clone()).unwrap();
                parse_version(&text).expect("torn swap")
            }
            None => parse_version(&on_disk).unwrap(),
        };
        assert!(newest >= acked, "edit {acked} lost, newest is {newest}");
    }
}
