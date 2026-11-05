use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_PROJECT_FILE_BYTES: u64 = 1024 * 1024;
const MAX_MAKE_TARGETS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunCommand {
    Argv(Vec<String>),
    Shell(String),
    UserShell(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunSource {
    UserConfig,
    Detected,
    CurrentFile,
    ProjectConfig,
    Typed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTarget {
    pub name: String,
    pub command: RunCommand,
    pub cwd: PathBuf,
    pub source: RunSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredRun {
    pub name: String,
    pub command: String,
    pub cwd: Option<String>,
    pub from_project: bool,
}

pub fn with_env(command: &str, env: &[(String, String)]) -> String {
    if env.is_empty() {
        return command.to_owned();
    }
    let pairs: Vec<String> = env
        .iter()
        .map(|(name, value)| format!("{name}={}", shell_quote(value)))
        .collect();
    format!("export {}; {command}", pairs.join(" "))
}

impl ConfiguredRun {
    pub fn target(&self, root: &Path) -> RunTarget {
        RunTarget {
            name: self.name.clone(),
            command: RunCommand::Shell(self.command.clone()),
            cwd: self
                .cwd
                .as_ref()
                .map_or_else(|| root.to_path_buf(), |c| root.join(c)),
            source: if self.from_project {
                RunSource::ProjectConfig
            } else {
                RunSource::UserConfig
            },
        }
    }
}

impl RunTarget {
    pub fn command_line(&self) -> String {
        match &self.command {
            RunCommand::Argv(argv) => argv
                .iter()
                .map(|a| shell_quote(a))
                .collect::<Vec<_>>()
                .join(" "),
            RunCommand::Shell(text) | RunCommand::UserShell(text) => text.clone(),
        }
    }

    pub fn label(&self) -> String {
        let command = self.command_line();
        let tag = match self.source {
            RunSource::ProjectConfig => "[project config] ",
            RunSource::UserConfig
            | RunSource::Detected
            | RunSource::CurrentFile
            | RunSource::Typed => "",
        };
        if self.name == command {
            return visible(&format!("{tag}{command}"));
        }
        visible(&format!("{tag}{}  —  {command}", self.name))
    }

    pub fn needs_confirmation(&self) -> bool {
        self.source == RunSource::ProjectConfig
    }
}

fn shell_quote(arg: &str) -> String {
    let plain = |c: char| c.is_alphanumeric() || "-_./:=@%+,".contains(c);
    if !arg.is_empty() && arg.chars().all(plain) {
        return arg.to_owned();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

fn is_deceptive(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

pub fn is_unprintable(c: char) -> bool {
    c.is_control() || is_deceptive(c)
}

pub fn visible(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if is_unprintable(c) => out.push_str(&c.escape_unicode().to_string()),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFacts {
    pub present: BTreeSet<String>,
    pub texts: BTreeMap<String, String>,
    pub programs: BTreeSet<String>,
    pub file: Option<ActiveFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveFile {
    pub path: PathBuf,
    pub executable: bool,
    pub shebang: bool,
}

#[derive(Clone, Copy)]
enum Commands {
    Fixed(&'static [&'static [&'static str]]),
    Wrapper {
        script: &'static str,
        program: &'static str,
        goals: &'static [&'static str],
    },
    Scripts(fn(&str, &BTreeSet<String>) -> Vec<Vec<String>>),
}

struct ProjectRule {
    markers: &'static [&'static str],
    commands: Commands,
}

const PROJECT_RULES: &[ProjectRule] = &[
    ProjectRule {
        markers: &["Cargo.toml"],
        commands: Commands::Fixed(&[&["cargo", "run"], &["cargo", "test"], &["cargo", "build"]]),
    },
    ProjectRule {
        markers: &["package.json"],
        commands: Commands::Scripts(package_scripts),
    },
    ProjectRule {
        markers: &["composer.json"],
        commands: Commands::Scripts(composer_scripts),
    },
    ProjectRule {
        markers: &["go.mod"],
        commands: Commands::Fixed(&[
            &["go", "run", "."],
            &["go", "test", "./..."],
            &["go", "build", "./..."],
        ]),
    },
    ProjectRule {
        markers: &["pom.xml"],
        commands: Commands::Wrapper {
            script: "mvnw",
            program: "mvn",
            goals: &["compile", "test", "package"],
        },
    },
    ProjectRule {
        markers: &["build.gradle", "build.gradle.kts"],
        commands: Commands::Wrapper {
            script: "gradlew",
            program: "gradle",
            goals: &["run", "test", "build"],
        },
    },
    ProjectRule {
        markers: &["pyproject.toml", "pytest.ini"],
        commands: Commands::Fixed(&[&["pytest"]]),
    },
    ProjectRule {
        markers: &["Makefile", "makefile", "GNUmakefile"],
        commands: Commands::Scripts(make_targets),
    },
];

const NODE_LOCKFILES: &[(&str, &str)] = &[
    ("bun.lock", "bun"),
    ("bun.lockb", "bun"),
    ("pnpm-lock.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("package-lock.json", "npm"),
];

struct FileRunner {
    extensions: &'static [&'static str],
    runners: &'static [&'static [&'static str]],
}

const FILE_RUNNERS: &[FileRunner] = &[
    FileRunner {
        extensions: &["sh"],
        runners: &[&["bash"], &["sh"]],
    },
    FileRunner {
        extensions: &["bash"],
        runners: &[&["bash"]],
    },
    FileRunner {
        extensions: &["zsh"],
        runners: &[&["zsh"]],
    },
    FileRunner {
        extensions: &["py"],
        runners: &[&["python3"], &["python"]],
    },
    FileRunner {
        extensions: &["php"],
        runners: &[&["php"]],
    },
    FileRunner {
        extensions: &["js", "mjs"],
        runners: &[&["node"], &["bun"]],
    },
    FileRunner {
        extensions: &["ts"],
        runners: &[
            &["./node_modules/.bin/tsx"],
            &["./node_modules/.bin/ts-node"],
            &["bun", "run"],
            &["deno", "run"],
            &["tsx"],
            &["ts-node"],
        ],
    },
    FileRunner {
        extensions: &["go"],
        runners: &[&["go", "run"]],
    },
];

pub fn probed_files() -> Vec<&'static str> {
    let mut files: Vec<&str> = PROJECT_RULES
        .iter()
        .flat_map(|r| r.markers.iter().copied())
        .collect();
    files.extend(NODE_LOCKFILES.iter().map(|(file, _)| *file));
    files.push("vendor/bin/phpunit");
    files.extend(PROJECT_RULES.iter().filter_map(|r| match r.commands {
        Commands::Wrapper { script, .. } => Some(script),
        _ => None,
    }));
    files.extend(FILE_RUNNERS.iter().flat_map(|f| {
        f.runners
            .iter()
            .filter_map(|r| r.first()?.strip_prefix("./"))
    }));
    files
}

pub fn read_files() -> Vec<&'static str> {
    PROJECT_RULES
        .iter()
        .filter(|r| matches!(r.commands, Commands::Scripts(_)))
        .flat_map(|r| r.markers.iter().copied())
        .collect()
}

pub fn probed_programs() -> Vec<&'static str> {
    FILE_RUNNERS
        .iter()
        .flat_map(|f| f.runners.iter().filter_map(|r| r.first().copied()))
        .filter(|p| !p.contains('/'))
        .collect()
}

fn owned(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|a| (*a).to_owned()).collect()
}

fn json_script_names(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    value
        .get("scripts")
        .and_then(Value::as_object)
        .map(|scripts| scripts.keys().cloned().collect())
        .unwrap_or_default()
}

fn package_scripts(text: &str, present: &BTreeSet<String>) -> Vec<Vec<String>> {
    let manager = NODE_LOCKFILES
        .iter()
        .find(|(file, _)| present.contains(*file))
        .map_or("npm", |(_, manager)| manager);
    json_script_names(text)
        .into_iter()
        .map(|name| vec![manager.to_owned(), "run".to_owned(), name])
        .collect()
}

fn composer_scripts(text: &str, _: &BTreeSet<String>) -> Vec<Vec<String>> {
    json_script_names(text)
        .into_iter()
        .map(|name| {
            if name == "test" {
                owned(&["composer", "test"])
            } else {
                vec!["composer".to_owned(), "run-script".to_owned(), name]
            }
        })
        .collect()
}

fn make_targets(text: &str, _: &BTreeSet<String>) -> Vec<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#', '.']) {
            continue;
        }
        let Some((names, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') || names.contains(['=', '%', '$', '?', '+']) {
            continue;
        }
        for name in names.split_whitespace() {
            if seen.insert(name.to_owned()) && targets.len() < MAX_MAKE_TARGETS {
                targets.push(vec!["make".to_owned(), name.to_owned()]);
            }
        }
    }
    targets
}

fn project_commands(rule: &ProjectRule, facts: &RunFacts) -> Vec<Vec<String>> {
    match rule.commands {
        Commands::Fixed(commands) => commands.iter().map(|c| owned(c)).collect(),
        Commands::Wrapper {
            script,
            program,
            goals,
        } => {
            let runner = if facts.present.contains(script) {
                format!("./{script}")
            } else {
                program.to_owned()
            };
            goals
                .iter()
                .map(|goal| vec![runner.clone(), (*goal).to_owned()])
                .collect()
        }
        Commands::Scripts(read) => rule
            .markers
            .iter()
            .find_map(|m| facts.texts.get(*m))
            .map(|text| read(text, &facts.present))
            .unwrap_or_default(),
    }
}

fn path_argument(path: &Path, root: &Path, explicit: bool) -> String {
    let Ok(relative) = path.strip_prefix(root) else {
        return path.display().to_string();
    };
    let text = relative.display().to_string();
    if explicit || text.starts_with('-') {
        format!("./{text}")
    } else {
        text
    }
}

fn file_target(root: &Path, facts: &RunFacts) -> Option<RunTarget> {
    let file = facts.file.as_ref()?;
    let name = file.path.file_name()?.to_string_lossy();
    let available = |runner: &&&[&str]| {
        runner.first().is_some_and(|program| {
            program.strip_prefix("./").map_or_else(
                || facts.programs.contains(*program),
                |path| facts.present.contains(path),
            )
        })
    };
    let argv = if file.executable && file.shebang {
        vec![path_argument(&file.path, root, true)]
    } else {
        let extension = file.path.extension()?.to_str()?;
        let rule = FILE_RUNNERS
            .iter()
            .find(|r| r.extensions.contains(&extension))?;
        let mut argv = owned(rule.runners.iter().find(available)?);
        argv.push(path_argument(&file.path, root, false));
        argv
    };
    Some(RunTarget {
        name: format!("Run {name}"),
        command: RunCommand::Argv(argv),
        cwd: if file.path.starts_with(root) {
            root.to_path_buf()
        } else {
            file.path.parent().unwrap_or(root).to_path_buf()
        },
        source: RunSource::CurrentFile,
    })
}

pub fn detect(root: &Path, facts: &RunFacts) -> Vec<RunTarget> {
    let mut targets: Vec<RunTarget> = PROJECT_RULES
        .iter()
        .filter(|r| r.markers.iter().any(|m| facts.present.contains(*m)))
        .flat_map(|r| project_commands(r, facts))
        .map(|argv| {
            let mut target = RunTarget {
                name: String::new(),
                command: RunCommand::Argv(argv),
                cwd: root.to_path_buf(),
                source: RunSource::Detected,
            };
            target.name = target.command_line();
            target
        })
        .collect();
    targets.extend(file_target(root, facts));
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/p";

    fn facts(present: &[&str], texts: &[(&str, &str)]) -> RunFacts {
        let mut facts = RunFacts {
            present: present.iter().map(|p| (*p).to_owned()).collect(),
            ..RunFacts::default()
        };
        for (name, text) in texts {
            facts.present.insert((*name).to_owned());
            facts.texts.insert((*name).to_owned(), (*text).to_owned());
        }
        facts
    }

    fn commands(facts: &RunFacts) -> Vec<String> {
        detect(Path::new(ROOT), facts)
            .iter()
            .map(RunTarget::command_line)
            .collect()
    }

    #[test]
    fn nothing_known_finds_nothing() {
        assert!(commands(&facts(&["README.md"], &[])).is_empty());
    }

    #[test]
    fn fixed_rules_list_their_commands() {
        assert_eq!(
            commands(&facts(&["Cargo.toml"], &[])),
            ["cargo run", "cargo test", "cargo build"]
        );
        assert_eq!(
            commands(&facts(&["go.mod"], &[])),
            ["go run .", "go test ./...", "go build ./..."]
        );
        assert_eq!(commands(&facts(&["pytest.ini"], &[])), ["pytest"]);
        assert_eq!(commands(&facts(&["pyproject.toml"], &[])), ["pytest"]);
    }

    #[test]
    fn a_project_with_two_build_files_gets_both() {
        assert_eq!(
            commands(&facts(&["go.mod", "pytest.ini"], &[])),
            ["go run .", "go test ./...", "go build ./...", "pytest"]
        );
    }

    #[test]
    fn package_scripts_use_the_manager_the_lockfile_implies() {
        let json = r#"{"scripts": {"dev": "vite", "test": "jest"}}"#;
        for (lock, expected) in [
            (None, "npm"),
            (Some("package-lock.json"), "npm"),
            (Some("yarn.lock"), "yarn"),
            (Some("pnpm-lock.yaml"), "pnpm"),
            (Some("bun.lockb"), "bun"),
            (Some("bun.lock"), "bun"),
        ] {
            let present: Vec<&str> = lock.into_iter().collect();
            assert_eq!(
                commands(&facts(&present, &[("package.json", json)])),
                [
                    format!("{expected} run dev"),
                    format!("{expected} run test")
                ]
            );
        }
    }

    #[test]
    fn broken_or_scriptless_json_finds_nothing_and_does_not_panic() {
        for text in [
            "{not json",
            "[]",
            r#"{"scripts": 3}"#,
            r#"{"name": "x"}"#,
            "",
        ] {
            assert!(commands(&facts(&[], &[("package.json", text)])).is_empty());
        }
    }

    #[test]
    fn composer_scripts_prefer_its_own_test_command() {
        let json = r#"{"scripts": {"lint": "phpcs", "test": "phpunit"}}"#;
        assert_eq!(
            commands(&facts(&[], &[("composer.json", json)])),
            ["composer run-script lint", "composer test"]
        );
    }

    #[test]
    fn java_builds_prefer_their_wrapper() {
        assert_eq!(
            commands(&facts(&["pom.xml"], &[])),
            ["mvn compile", "mvn test", "mvn package"]
        );
        assert_eq!(
            commands(&facts(&["pom.xml", "mvnw"], &[])),
            ["./mvnw compile", "./mvnw test", "./mvnw package"]
        );
        assert_eq!(
            commands(&facts(&["build.gradle.kts"], &[])),
            ["gradle run", "gradle test", "gradle build"]
        );
        assert_eq!(
            commands(&facts(&["build.gradle", "gradlew"], &[])),
            ["./gradlew run", "./gradlew test", "./gradlew build"]
        );
    }

    #[test]
    fn make_targets_are_read_simply() {
        let text = "CC := gcc\nX = a:b\nY ?= 1\n.PHONY: all\n# c: d\nall: main.o\n\tgcc: not me\n%.o: %.c\nbuild test: all\nall:\n$(OUT): x\n";
        assert_eq!(
            commands(&facts(&[], &[("Makefile", text)])),
            ["make all", "make build", "make test"]
        );
    }

    fn with_file(path: &str, programs: &[&str], executable: bool, shebang: bool) -> RunFacts {
        let mut f = facts(&[], &[]);
        f.programs = programs.iter().map(|p| (*p).to_owned()).collect();
        f.file = Some(ActiveFile {
            path: PathBuf::from(path),
            executable,
            shebang,
        });
        f
    }

    #[test]
    fn a_file_runs_with_the_first_runner_that_exists() {
        for (path, programs, expected) in [
            ("/p/a.sh", &["bash", "sh"][..], "bash a.sh"),
            ("/p/a.sh", &["sh"], "sh a.sh"),
            ("/p/a.bash", &["bash"], "bash a.bash"),
            ("/p/a.zsh", &["zsh"], "zsh a.zsh"),
            ("/p/sub/a.py", &["python", "python3"], "python3 sub/a.py"),
            ("/p/a.py", &["python"], "python a.py"),
            ("/p/a.php", &["php"], "php a.php"),
            ("/p/a.js", &["node"], "node a.js"),
            ("/p/a.mjs", &["node"], "node a.mjs"),
            ("/p/a.go", &["go"], "go run a.go"),
            ("/p/a.ts", &["deno"], "deno run a.ts"),
            ("/p/-x.py", &["python3"], "python3 ./-x.py"),
            ("/p/my a.py", &["python3"], "python3 'my a.py'"),
        ] {
            assert_eq!(
                commands(&with_file(path, programs, false, false)),
                [expected]
            );
        }
    }

    #[test]
    fn typescript_needs_a_runner_and_rust_is_not_a_script() {
        assert!(commands(&with_file("/p/a.ts", &["node"], false, false)).is_empty());
        assert!(commands(&with_file("/p/a.rs", &["rustc"], false, false)).is_empty());
        assert!(commands(&with_file("/p/noext", &["bash"], false, false)).is_empty());
    }

    #[test]
    fn the_projects_own_typescript_runner_comes_first() {
        let mut f = with_file("/p/a.ts", &["tsx", "bun"], false, false);
        f.present.insert("node_modules/.bin/tsx".to_owned());
        assert_eq!(commands(&f), ["./node_modules/.bin/tsx a.ts"]);
        assert!(probed_files().contains(&"node_modules/.bin/tsx"));
    }

    #[test]
    fn a_shebang_with_the_executable_bit_runs_directly() {
        assert_eq!(commands(&with_file("/p/tool", &[], true, true)), ["./tool"]);
        assert_eq!(
            commands(&with_file("/p/a.py", &["python3"], true, true)),
            ["./a.py"]
        );
        assert!(commands(&with_file("/p/tool", &[], false, true)).is_empty());
        assert!(commands(&with_file("/p/tool", &[], true, false)).is_empty());
    }

    #[test]
    fn a_file_outside_the_project_runs_from_its_own_folder() {
        let targets = detect(
            Path::new(ROOT),
            &with_file("/elsewhere/a.py", &["python3"], false, false),
        );
        assert_eq!(targets[0].cwd, PathBuf::from("/elsewhere"));
        assert_eq!(targets[0].command_line(), "python3 /elsewhere/a.py");
    }

    #[test]
    fn everything_the_tables_use_is_probed() {
        let files = probed_files();
        for name in ["Cargo.toml", "mvnw", "gradlew", "yarn.lock", "go.mod"] {
            assert!(files.contains(&name), "{name}");
        }
        let read = read_files();
        for name in ["package.json", "composer.json", "Makefile"] {
            assert!(read.contains(&name), "{name}");
        }
        assert!(!read.contains(&"Cargo.toml"));
        let programs = probed_programs();
        assert!(programs.contains(&"python3") && programs.contains(&"go"));
        assert!(!programs.iter().any(|p| p.contains('/')));
    }

    #[test]
    fn quoting_shows_a_command_that_can_be_pasted() {
        let target = RunTarget {
            name: "x".to_owned(),
            command: RunCommand::Argv(owned(&["a", "b c", "it's", ""])),
            cwd: PathBuf::from(ROOT),
            source: RunSource::Detected,
        };
        assert_eq!(target.command_line(), r"a 'b c' 'it'\''s' ''");
    }

    #[test]
    fn nothing_in_a_label_can_hide_the_rest_of_a_command() {
        let entry = ConfiguredRun {
            name: "bu\u{1b}[2Jild\n".to_owned(),
            command: "true\n\u{202e}rm -rf ~".to_owned(),
            cwd: None,
            from_project: true,
        };
        let target = entry.target(Path::new(ROOT));
        let label = target.label();
        assert!(!label.chars().any(is_unprintable), "{label:?}");
        assert!(label.contains("rm -rf ~"), "{label:?}");
        assert!(label.starts_with("[project config] "), "{label:?}");
        assert!(target.needs_confirmation());
    }

    #[test]
    fn the_config_decides_where_a_command_runs_and_how_far_it_is_trusted() {
        let mut entry = ConfiguredRun {
            name: "x".to_owned(),
            command: "true".to_owned(),
            cwd: Some("sub".to_owned()),
            from_project: false,
        };
        let target = entry.target(Path::new(ROOT));
        assert_eq!(target.cwd, PathBuf::from("/p/sub"));
        assert_eq!(target.source, RunSource::UserConfig);
        assert!(!target.needs_confirmation());
        entry.cwd = None;
        assert_eq!(entry.target(Path::new(ROOT)).cwd, PathBuf::from(ROOT));
        assert_eq!(target.label(), "x  —  true");
    }
}
