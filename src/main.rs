use std::path::PathBuf;
use std::process::ExitCode;

use ttywright::app::{self, Target, WindowKind};
use ttywright::services::run::{LAUNCHER_FLAG, exec_in_new_session};

const USAGE: &str = "usage: ttyw [project-dir | file]\n       ttyw mcp [project-dir]\n       ttyw --window git-log|run [project-dir]";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().is_some_and(|a| a == LAUNCHER_FLAG) {
        args.next();
        return exec_in_new_session(args);
    }
    #[cfg(feature = "static-modules")]
    if args
        .peek()
        .is_some_and(|a| a == ttywright::services::plugin::PLUGIN_HOST_FLAG)
    {
        return ttywright_plugin_host::serve();
    }
    if args.peek().is_some_and(|a| a == "mcp") {
        args.next();
        let start = args
            .next()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok());
        let Some(start) = start else {
            eprintln!("ttywright: cannot read the current directory");
            return ExitCode::FAILURE;
        };
        ttywright::services::mcp::serve(start);
        return ExitCode::SUCCESS;
    }
    let mut window = None;
    if args.peek().is_some_and(|a| a == "--window") {
        args.next();
        window = match args.next().as_deref().and_then(|kind| kind.to_str()) {
            Some("git-log") => Some(WindowKind::GitLog),
            Some("run") => Some(WindowKind::Run),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        };
    }
    let (arg, None) = (args.next(), args.next()) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if arg.as_deref().is_some_and(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("ttywright: cannot read the current directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut target = Target::from_arg(arg.map(PathBuf::from), &cwd);
    target.window = window;
    match app::run(target) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ttywright: {e:#}");
            ExitCode::FAILURE
        }
    }
}
