use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

fn tools_for(path: &Path) -> &'static [(&'static str, &'static [&'static str])] {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => &[("rustfmt", &["--edition", "2024"])],
        "go" => &[("gofmt", &[])],
        "py" | "pyi" => &[
            ("ruff", &["format", "--stdin-filename", "{path}", "-"]),
            ("black", &["-q", "--stdin-filename", "{path}", "-"]),
        ],
        "sh" | "bash" | "zsh" => &[("shfmt", &["--filename", "{path}"])],
        "java" => &[("google-java-format", &["-"])],
        "php" => &[("prettier", &["--stdin-filepath", "{path}"])],
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "json" | "css" | "scss"
        | "html" | "vue" | "md" | "yml" | "yaml" => {
            &[("prettier", &["--stdin-filepath", "{path}"])]
        }
        _ => &[],
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

pub fn format_text(path: &Path, root: &Path, text: &str) -> Result<String, String> {
    let tools = tools_for(path);
    let Some((program, args)) = tools.iter().find(|(program, _)| on_path(program)) else {
        if tools.is_empty() {
            return Err("No formatter is known for this file".to_owned());
        }
        let names: Vec<&str> = tools.iter().map(|(program, _)| *program).collect();
        return Err(format!(
            "No language server to format, and {} is not installed",
            names.join(" or ")
        ));
    };
    let shown = path.display().to_string();
    let args: Vec<&str> = args
        .iter()
        .map(|arg| {
            if *arg == "{path}" {
                shown.as_str()
            } else {
                arg
            }
        })
        .collect();
    run_tool(program, &args, root, text)
}

fn run_tool(program: &str, args: &[&str], cwd: &Path, text: &str) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let input = text.to_owned();
    std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let killer_id = child.id();
    let (done_tx, done) = channel();
    std::thread::spawn(move || {
        let _ = done_tx.send(child.wait_with_output());
    });
    let output = match done.recv_timeout(TIMEOUT) {
        Ok(output) => output.map_err(|e| format!("{program}: {e}"))?,
        Err(_) => {
            let _ = Command::new("kill").arg(killer_id.to_string()).status();
            return Err(format!("{program} took too long"));
        }
    };
    if !output.status.success() {
        let complaint = String::from_utf8_lossy(&output.stderr);
        let first = complaint
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed");
        return Err(format!("{program}: {}", crate::core::run::visible(first)));
    }
    let formatted = String::from_utf8(output.stdout).map_err(|_| format!("{program}: not text"))?;
    if formatted.is_empty() && !text.is_empty() {
        return Err(format!("{program} gave back nothing"));
    }
    Ok(formatted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, text: &str) -> Result<String, String> {
        run_tool("sh", &["-c", script], Path::new("/"), text)
    }

    #[test]
    fn a_tool_s_output_replaces_the_text_only_if_it_worked() {
        assert_eq!(sh("tr a b", "aa\n").unwrap(), "bb\n");
        let failed = sh("echo 'bad syntax' >&2; echo partial; exit 1", "x").unwrap_err();
        assert_eq!(failed, "sh: bad syntax");
        assert!(sh("cat >/dev/null", "x").unwrap_err().contains("nothing"));
    }

    #[test]
    fn big_text_does_not_block_on_full_pipes() {
        let big = "line\n".repeat(200_000);
        assert_eq!(sh("cat", &big).unwrap().len(), big.len());
    }

    #[test]
    fn unknown_files_and_missing_tools_are_explained() {
        let root = Path::new("/");
        assert!(
            format_text(Path::new("a.txt"), root, "x")
                .unwrap_err()
                .contains("known")
        );
        assert!(tools_for(Path::new("a.py")).iter().any(|t| t.0 == "ruff"));
        assert!(
            tools_for(Path::new("a.php"))
                .iter()
                .any(|t| t.0 == "prettier")
        );
    }
}
