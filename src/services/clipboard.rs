use std::io::Write;
use std::process::{Command, Stdio};

const OSC52_MAX_BYTES: usize = 74_994;

const COPY_TOOLS: [(&str, &[&str]); 3] = [
    ("pbcopy", &[]),
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
];
const PASTE_TOOLS: [(&str, &[&str]); 3] = [
    ("pbpaste", &[]),
    ("wl-paste", &["--no-newline"]),
    ("xclip", &["-selection", "clipboard", "-o"]),
];

fn over_ssh() -> bool {
    std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = u32::from(chunk[0]);
        let b = u32::from(chunk.get(1).copied().unwrap_or(0));
        let c = u32::from(chunk.get(2).copied().unwrap_or(0));
        let n = a << 16 | b << 8 | c;
        let digit = |shift: u32| char::from(ALPHABET[(n >> shift & 63) as usize]);
        out.push(digit(18));
        out.push(digit(12));
        out.push(if chunk.len() > 1 { digit(6) } else { '=' });
        out.push(if chunk.len() > 2 { digit(0) } else { '=' });
    }
    out
}

fn osc52(text: &str) -> Option<String> {
    (text.len() <= OSC52_MAX_BYTES).then(|| format!("\x1b]52;c;{}\x07", base64(text.as_bytes())))
}

fn copy_with_tool(text: &str) {
    for (tool, args) in COPY_TOOLS {
        let child = Command::new(tool)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            std::thread::spawn(move || child.wait());
            return;
        }
    }
}

pub fn copy(text: &str) {
    if over_ssh() {
        if let Some(sequence) = osc52(text) {
            let mut out = std::io::stdout();
            let _ = out.write_all(sequence.as_bytes());
            let _ = out.flush();
        }
    } else {
        let text = text.to_owned();
        std::thread::spawn(move || copy_with_tool(&text));
    }
}

pub fn paste() -> Option<String> {
    PASTE_TOOLS.into_iter().find_map(|(tool, args)| {
        let output = Command::new(tool)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("ы", "0Ys="),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn huge_text_is_not_sent_to_the_terminal() {
        assert_eq!(osc52("hi").as_deref(), Some("\x1b]52;c;aGk=\x07"));
        assert_eq!(osc52(&"x".repeat(OSC52_MAX_BYTES + 1)), None);
    }
}
