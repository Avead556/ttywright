#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

const ROWS: u16 = 44;
const COLS: u16 = 150;

struct Session {
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
    parser: vt100::Parser,
    _master: Box<dyn MasterPty + Send>,
}

impl Session {
    fn start(program: &Path, home: &Path, project: &Path, args: &[&str]) -> Self {
        Self::start_with_env(program, home, project, args, &[])
    }

    fn start_with_env(
        program: &Path,
        home: &Path,
        project: &Path,
        args: &[&str],
        extra: &[(&str, String)],
    ) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.cwd(project);
        let real_home = std::env::var("HOME").unwrap();
        command.env(
            "RUSTUP_HOME",
            std::env::var("RUSTUP_HOME").unwrap_or(format!("{real_home}/.rustup")),
        );
        command.env(
            "CARGO_HOME",
            std::env::var("CARGO_HOME").unwrap_or(format!("{real_home}/.cargo")),
        );
        command.env("HOME", home);
        command.env("XDG_DATA_HOME", home.join("data"));
        command.env("XDG_CONFIG_HOME", home.join("config"));
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TTYWRIGHT_TRUST_ALL", "1");
        for (name, _) in std::env::vars() {
            if name.starts_with("CLAUDE") {
                command.env_remove(name);
            }
        }
        for (name, value) in extra {
            command.env(name, value);
        }
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let (tx, output) = channel();
        std::thread::spawn(move || {
            let mut chunk = [0; 65536];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 || tx.send(chunk[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            writer,
            output,
            parser: vt100::Parser::new(ROWS, COLS, 0),
            _master: pair.master,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn pump(&mut self, time: Duration) {
        let deadline = Instant::now() + time;
        while let Ok(chunk) = self
            .output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            if chunk.windows(3).any(|w| w == b"\x1b[c") {
                self.send(b"\x1b[?62c");
            }
            if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                self.send(b"\x1b[1;1R");
            }
            self.parser.process(&chunk);
        }
    }

    fn wait_for(&mut self, wanted: &str, time: Duration) {
        let deadline = Instant::now() + time;
        while Instant::now() < deadline {
            if self.parser.screen().contents().contains(wanted) {
                self.pump(Duration::from_millis(400));
                return;
            }
            self.pump(Duration::from_millis(100));
        }
        panic!(
            "never saw {wanted:?} in\n{}",
            self.parser.screen().contents()
        );
    }

    fn wait_settled(&mut self, at_least: Duration, still: Duration, time: Duration) {
        let start = Instant::now();
        let mut last = String::new();
        let mut since = Instant::now();
        while start.elapsed() < time {
            self.pump(Duration::from_millis(500));
            let now = self.parser.screen().contents();
            if now != last {
                last = now;
                since = Instant::now();
            } else if start.elapsed() >= at_least && since.elapsed() >= still {
                return;
            }
        }
        panic!("the screen never settled:\n{last}");
    }

    fn type_slowly(&mut self, text: &str) {
        for c in text.chars() {
            self.send(c.to_string().as_bytes());
            self.pump(Duration::from_millis(30));
        }
    }

    fn keys(&mut self, bytes: &[u8]) {
        self.send(bytes);
        self.pump(Duration::from_millis(600));
    }

    fn quit(mut self) {
        self.send(b"\x11");
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            self.pump(Duration::from_millis(100));
        }
        self.child.kill().ok();
    }
}

fn palette(index: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0x1e, 0x1f, 0x22),
        (0xf7, 0x54, 0x64),
        (0x6a, 0xab, 0x73),
        (0xe5, 0xc0, 0x7b),
        (0x56, 0xa8, 0xf5),
        (0xc7, 0x7d, 0xbb),
        (0x2a, 0xac, 0xb8),
        (0xbc, 0xbe, 0xc4),
        (0x6f, 0x73, 0x7a),
        (0xff, 0x6b, 0x7a),
        (0x8c, 0xc5, 0x8f),
        (0xf2, 0xd3, 0x8e),
        (0x7a, 0xbc, 0xff),
        (0xd9, 0x96, 0xd0),
        (0x4f, 0xc6, 0xd0),
        (0xdf, 0xe1, 0xe5),
    ];
    match index {
        0..=15 => BASE[usize::from(index)],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
    }
}

fn css(color: vt100::Color, fallback: (u8, u8, u8)) -> String {
    let (r, g, b) = match color {
        vt100::Color::Default => fallback,
        vt100::Color::Idx(i) => palette(i),
        vt100::Color::Rgb(r, g, b) => (r, g, b),
    };
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

const FG: (u8, u8, u8) = (0xbc, 0xbe, 0xc4);
const BG: (u8, u8, u8) = (0x1e, 0x1f, 0x22);

fn html(screen: &vt100::Screen, title: &str) -> String {
    let mut rows = String::new();
    for row in 0..ROWS {
        rows.push_str("<div class=\"row\">");
        let mut col = 0;
        while col < COLS {
            let Some(cell) = screen.cell(row, col) else {
                col += 1;
                continue;
            };
            let (mut fg, mut bg) = (css(cell.fgcolor(), FG), css(cell.bgcolor(), BG));
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let text = if cell.has_contents() {
                cell.contents().to_owned()
            } else {
                " ".to_owned()
            };
            let wide = cell.is_wide();
            let mut style = format!("color:{fg};background:{bg};");
            if cell.bold() {
                style.push_str("font-weight:700;");
            }
            if cell.italic() {
                style.push_str("font-style:italic;");
            }
            if cell.underline() {
                style.push_str("text-decoration:underline;");
            }
            let width = if wide { 2 } else { 1 };
            let _ = write!(
                rows,
                "<span class=\"c{width}\" style=\"{style}\">{}</span>",
                escape(&text)
            );
            col += width;
        }
        rows.push_str("</div>");
    }
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<link href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:ital,wght@0,400;0,700;1,400&display=block" rel="stylesheet">
<style>
html,body{{margin:0;background:transparent}}
body{{padding:28px}}
.window{{display:inline-block;border-radius:12px;overflow:hidden;box-shadow:0 22px 60px rgba(0,0,0,.45),0 0 0 1px rgba(255,255,255,.08);background:#1e1f22}}
.bar{{height:34px;background:#2b2d30;display:flex;align-items:center;padding:0 14px;gap:8px;position:relative}}
.dot{{width:12px;height:12px;border-radius:50%}}
.title{{position:absolute;left:0;right:0;text-align:center;color:#9da0a8;font:13px -apple-system,BlinkMacSystemFont,sans-serif}}
.screen{{padding:6px 8px 10px;font-family:'JetBrains Mono',Menlo,monospace;font-size:14px;line-height:19px;font-variant-ligatures:none}}
.row{{height:19px;white-space:pre;display:flex}}
.row span{{display:inline-block;height:19px;overflow:visible}}
.c1{{width:8.4px}}.c2{{width:16.8px}}
</style></head><body><div class="window"><div class="bar"><div class="dot" style="background:#ff5f57"></div><div class="dot" style="background:#febc2e"></div><div class="dot" style="background:#28c840"></div><div class="title">{title}</div></div><div class="screen">{rows}</div></div></body></html>"#
    )
}

fn write(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    git_as(dir, "Ada Lovelace", args);
}

fn git_as(dir: &Path, author: &str, args: &[&str]) {
    let email = format!(
        "{}@example.com",
        author.split(' ').next().unwrap().to_lowercase()
    );
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_AUTHOR_EMAIL", &email)
        .env("GIT_COMMITTER_NAME", author)
        .env("GIT_COMMITTER_EMAIL", &email)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn cargo(dir: &Path, args: &[&str]) {
    let status = Command::new("cargo")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "cargo {args:?}");
}

const CARGO_TOML: &str = r#"[package]
name = "inventory"
version = "0.1.0"
edition = "2024"
"#;

const MAIN_RS: &str = r#"mod store;

use store::{Item, Store};

fn main() {
    let mut store = Store::default();
    store.add(Item::new("keyboard", 3, 89.0));
    store.add(Item::new("monitor", 1, 249.0));
    store.add(Item::new("cable", 12, 4.5));

    for item in store.low_stock(2) {
        println!("restock {} (only {} left)", item.name, item.count);
    }
    println!("total value: {:.2}", store.value());
}
"#;

const STORE_V1: &str = r#"use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
    pub count: u32,
    pub price: f64,
}

impl Item {
    pub fn new(name: &str, count: u32, price: f64) -> Self {
        Self {
            name: name.to_owned(),
            count,
            price,
        }
    }
}

#[derive(Debug, Default)]
pub struct Store {
    items: BTreeMap<String, Item>,
}

impl Store {
    pub fn add(&mut self, item: Item) {
        self.items.insert(item.name.clone(), item);
    }

    pub fn value(&self) -> f64 {
        self.items.values().map(|i| f64::from(i.count) * i.price).sum()
    }
}
"#;

const STORE_V2: &str = r#"use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
    pub count: u32,
    pub price: f64,
}

impl Item {
    pub fn new(name: &str, count: u32, price: f64) -> Self {
        Self {
            name: name.to_owned(),
            count,
            price,
        }
    }
}

#[derive(Debug, Default)]
pub struct Store {
    items: BTreeMap<String, Item>,
}

impl Store {
    pub fn add(&mut self, item: Item) {
        let entry = self.items.entry(item.name.clone()).or_insert(Item {
            count: 0,
            ..item.clone()
        });
        entry.count += item.count;
        entry.price = item.price;
    }

    pub fn low_stock(&self, limit: u32) -> impl Iterator<Item = &Item> {
        self.items.values().filter(move |i| i.count <= limit)
    }

    pub fn value(&self) -> f64 {
        self.items.values().map(|i| f64::from(i.count) * i.price).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_the_same_item_twice_adds_up() {
        let mut store = Store::default();
        store.add(Item::new("cable", 2, 4.5));
        store.add(Item::new("cable", 3, 4.5));
        assert_eq!(store.low_stock(10).next().map(|i| i.count), Some(5));
    }

    #[test]
    fn the_value_counts_every_item() {
        let mut store = Store::default();
        store.add(Item::new("monitor", 2, 100.0));
        assert_eq!(store.value(), 200.0);
    }
}
"#;

const README_MD: &str = "# inventory\n\nA tiny store that knows what to restock.\n";

fn demo_project(root: &Path) -> PathBuf {
    let project = root.join("inventory");
    write(&project.join("Cargo.toml"), CARGO_TOML);
    write(&project.join(".gitignore"), "/target\n");
    write(&project.join("src/main.rs"), "fn main() {}\n");
    cargo(&project, &["generate-lockfile", "--offline"]);
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["add", "."]);
    git(
        &project,
        &["commit", "-q", "-m", "Start the inventory crate"],
    );
    write(&project.join("src/store.rs"), STORE_V1);
    git_as(&project, "Grace Hopper", &["add", "."]);
    git_as(
        &project,
        "Grace Hopper",
        &["commit", "-q", "-m", "Keep items in a store"],
    );
    write(&project.join("src/main.rs"), MAIN_RS);
    git(&project, &["commit", "-qam", "Print what needs restocking"]);
    write(&project.join("README.md"), README_MD);
    git_as(&project, "Linus Torvalds", &["add", "."]);
    git_as(
        &project,
        "Linus Torvalds",
        &["commit", "-q", "-m", "Add a README"],
    );
    write(
        &project.join("README.md"),
        &format!("{README_MD}\nRun `cargo run` to see what is low.\n"),
    );
    git_as(
        &project,
        "Grace Hopper",
        &["commit", "-qam", "Say how to run it"],
    );
    write(&project.join("src/store.rs"), STORE_V2);
    write(
        &project.join("CHANGELOG.md"),
        "# Changelog\n\n- Items with the same name add up\n- `low_stock` lists what to reorder\n",
    );
    cargo(&project, &["build", "--quiet"]);
    cargo(&project, &["test", "--no-run", "--quiet"]);
    project
}

fn shot(session: &Session, out: &Path, name: &str, title: &str) {
    write(
        &out.join(format!("{name}.html")),
        &html(session.parser.screen(), title),
    );
    println!("{name}");
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("docs/screenshots"), PathBuf::from);
    let program = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("ttyw");
    assert!(program.is_file(), "build ttyw first: {}", program.display());
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let project = demo_project(&home_path.join("projects"));
    let title = "ttyw — inventory";

    let mut s = Session::start(&program, &home_path, &project, &["src/store.rs"]);
    s.wait_for("low_stock", Duration::from_secs(30));
    s.keys(b"\x1b1");
    s.wait_for("Cargo.toml", Duration::from_secs(10));
    s.keys(b"\x1b[B\x1b[C");
    s.keys(b"\x1b");
    s.pump(Duration::from_secs(12));
    s.keys(b"\x1b[39;1H");
    shot(&s, &out, "editor", title);

    s.keys(b"\x0b");
    s.keys(b"git");
    shot(&s, &out, "palette", title);
    s.keys(b"\x1b");

    s.keys(b"\x1b0");
    s.wait_for("CHANGELOG.md", Duration::from_secs(10));
    s.keys(b"\x1b[B");
    s.pump(Duration::from_secs(2));
    shot(&s, &out, "git", title);
    s.keys(b"\x1b");

    s.keys(b"\x1bx");
    s.keys(b"cargo test\r");
    s.wait_for("test result", Duration::from_secs(60));
    shot(&s, &out, "run", title);

    s.keys(b"\x1b9");
    s.wait_for("Say how to run it", Duration::from_secs(10));
    s.pump(Duration::from_secs(2));
    shot(&s, &out, "log", title);
    s.quit();

    if std::env::var_os("SCREENSHOT_CLAUDE").is_some() {
        claude_shot(&program, &home_path, &project, &out, title);
    }
}

fn claude_shot(program: &Path, home: &Path, project: &Path, out: &Path, title: &str) {
    let found = Command::new("sh")
        .args(["-c", "command -v claude"])
        .output()
        .unwrap();
    let claude = String::from_utf8(found.stdout).unwrap().trim().to_owned();
    assert!(!claude.is_empty(), "claude is not installed");
    let real_home = std::env::var("HOME").unwrap();
    let bin = home.join("bin");
    write(
        &bin.join("claude"),
        &format!(
            "#!/bin/sh\nunset XDG_DATA_HOME XDG_CONFIG_HOME CLAUDE_CONFIG_DIR\nDISABLE_AUTOUPDATER=1 HOME='{real_home}' exec '{claude}' --settings '{{\"statusLine\":{{\"type\":\"command\",\"command\":\"true\"}}}}' \"$@\"\n"
        ),
    );
    Command::new("chmod")
        .arg("+x")
        .arg(bin.join("claude"))
        .status()
        .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let env = [
        ("PATH", path),
        ("CLAUDE_CONFIG_DIR", format!("{real_home}/.claude")),
    ];
    let mut s = Session::start_with_env(program, home, project, &["src/store.rs"], &env);
    s.wait_for("BTreeMap", Duration::from_secs(30));
    s.keys(b"\x1b2");
    for _ in 0..8 {
        s.keys(b"\x1b=");
    }
    s.pump(Duration::from_secs(8));
    if s.parser.screen().contents().contains("trust this folder") {
        s.keys(b"\x1b[B");
        s.keys(b"\r");
        s.pump(Duration::from_secs(5));
    }
    s.wait_for("Claude Code v", Duration::from_secs(60));
    s.type_slowly("/clear");
    s.keys(b"\r");
    s.pump(Duration::from_secs(3));
    s.type_slowly(
        "In one short paragraph: what does low_stock in src/store.rs do, and is there a bug?",
    );
    s.keys(b"\r");
    s.wait_settled(
        Duration::from_secs(15),
        Duration::from_secs(8),
        Duration::from_secs(240),
    );
    shot(&s, out, "claude", title);
    s.send(b"\x1b2");
    s.quit();
}
