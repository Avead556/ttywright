use std::path::Path;
use std::time::{Duration, Instant};

use super::analysis::on_path;
use super::plugin::{RunRequest, run_program};
use crate::core::cli::{CliOutcome, CliTool, clean_block, redact};

const MAX_BYTES: usize = 32 << 20;
const READ_LIMIT: Duration = Duration::from_secs(60);
const WRITE_LIMIT: Duration = Duration::from_secs(300);

const GH_ENV: [(&str, &str); 8] = [
    ("GH_PROMPT_DISABLED", "1"),
    ("GH_NO_UPDATE_NOTIFIER", "1"),
    ("GH_SPINNER_DISABLED", "1"),
    ("GH_PAGER", "cat"),
    ("PAGER", "cat"),
    ("NO_COLOR", "1"),
    ("CLICOLOR", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
];

pub struct CliRequest<'a> {
    pub tool: CliTool,
    pub args: Vec<String>,
    pub input: Option<String>,
    pub root: &'a Path,
    pub in_project: bool,
    pub write: bool,
    pub launcher: Option<&'a Path>,
}

pub fn run(request: CliRequest<'_>) -> CliOutcome {
    let Some(program) = on_path(request.tool.program()) else {
        return CliOutcome::Missing;
    };
    let limit = if request.write {
        WRITE_LIMIT
    } else {
        READ_LIMIT
    };
    let env = match request.tool {
        CliTool::Gh => &GH_ENV[..],
        CliTool::Docker => &[],
    };
    let result = run_program(
        RunRequest {
            program: program.display().to_string(),
            args: request.args,
            stdin: request.input,
            cwd: Some(if request.in_project {
                request.root.to_path_buf()
            } else {
                std::env::temp_dir()
            }),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        },
        request.root,
        Instant::now() + limit,
        MAX_BYTES,
        request.launcher,
    );
    if result.timed_out {
        return CliOutcome::TimedOut;
    }
    if result.truncated {
        return CliOutcome::TooLong;
    }
    if let Some(error) = result.error {
        return CliOutcome::Failed(said(&error));
    }
    if result.code == Some(0) {
        return CliOutcome::Done(result.stdout);
    }
    CliOutcome::Failed(said(if result.stderr.trim().is_empty() {
        &result.stdout
    } else {
        &result.stderr
    }))
}

fn said(text: &str) -> String {
    clean_block(&redact(text)).join("\n")
}
