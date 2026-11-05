use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::provision::Download;
use super::{Bundled, LspConfig, bundled};
use crate::config::RustServer;
use crate::core::{LspServerOption, ServerChoice};
use crate::services::fs::write_atomic;
use crate::services::paths;

const RELEASE_PAGES: [u32; 2] = [1, 2];
const LIST_TIMEOUT_SECONDS: &str = "10";
const NO_COMMAND: &str = "";

pub fn load(file: &Path) -> ServerChoice {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(file: &Path, choice: &ServerChoice) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_atomic(file, &serde_json::to_vec(choice)?)
}

pub fn config_for(choice: &ServerChoice, root: PathBuf, settings: &RustServer) -> LspConfig {
    if let Ok(command) = std::env::var("TTYWRIGHT_RUST_LSP") {
        return LspConfig::new(command, root);
    }
    if let Some(command) = &settings.command {
        let mut config = LspConfig::new(command.clone(), root);
        config.args.clone_from(&settings.args);
        return config;
    }
    let command = match choice {
        ServerChoice::Automatic | ServerChoice::FromPath => "rust-analyzer",
        ServerChoice::Bundled | ServerChoice::Release(_) => NO_COMMAND,
    };
    let mut config = LspConfig::new(command.to_owned(), root);
    config.check_first = true;
    let Some((unpacked, downloaded)) = paths::servers_dirs() else {
        return config;
    };
    let embedded = bundled::embedded().map(|compressed| Bundled {
        compressed,
        dir: unpacked,
    });
    match choice {
        ServerChoice::Automatic => {
            if settings.auto_download {
                config.download = Some(Download::new(downloaded));
            }
            config.bundled = embedded;
            config.bundled_file = crate::modules::rust_analyzer();
        }
        ServerChoice::FromPath => {}
        ServerChoice::Bundled => {
            config.bundled = embedded;
            config.bundled_file = crate::modules::rust_analyzer();
        }
        ServerChoice::Release(tag) => {
            let mut download = Download::new(downloaded);
            download.pinned = Some(tag.clone());
            config.download = Some(download);
        }
    }
    config
}

fn parse_release_tags(body: &[u8]) -> Vec<String> {
    let Ok(Value::Array(releases)) = serde_json::from_slice(body) else {
        return Vec::new();
    };
    releases
        .iter()
        .filter_map(|r| r.get("tag_name")?.as_str())
        .filter(|tag| tag.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_owned)
        .collect()
}

fn fetch_release_tags() -> Vec<String> {
    let page = |n: u32| {
        let url = format!(
            "https://api.github.com/repos/rust-lang/rust-analyzer/releases?per_page=100&page={n}"
        );
        Command::new("curl")
            .args(["--fail", "--location", "--silent", "--max-time"])
            .arg(LIST_TIMEOUT_SECONDS)
            .args(["--header", "User-Agent: ttywright"])
            .arg(url)
            .output()
            .map(|output| parse_release_tags(&output.stdout))
            .unwrap_or_default()
    };
    RELEASE_PAGES.into_iter().flat_map(page).collect()
}

fn downloaded_tags() -> BTreeSet<String> {
    let Some((_, dir)) = paths::servers_dirs() else {
        return BTreeSet::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().join("rust-analyzer").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

fn build_options(
    current: &ServerChoice,
    has_bundled: bool,
    downloaded: &BTreeSet<String>,
    releases: Vec<String>,
) -> Vec<LspServerOption> {
    let mut tags: BTreeSet<String> = releases.into_iter().collect();
    tags.extend(downloaded.iter().cloned());
    let mut choices = vec![
        (
            "Automatic: match the project's Rust".to_owned(),
            ServerChoice::Automatic,
        ),
        ("rust-analyzer from PATH".to_owned(), ServerChoice::FromPath),
    ];
    if has_bundled {
        choices.push(("Built into ttywright".to_owned(), ServerChoice::Bundled));
    }
    choices.extend(tags.into_iter().rev().map(|tag| {
        let note = if downloaded.contains(&tag) {
            " (downloaded)"
        } else {
            ""
        };
        (format!("{tag}{note}"), ServerChoice::Release(tag))
    }));
    choices
        .into_iter()
        .map(|(label, choice)| LspServerOption {
            current: choice == *current,
            label,
            choice,
        })
        .collect()
}

pub fn list_options(current: &ServerChoice) -> Vec<LspServerOption> {
    build_options(
        current,
        bundled::embedded().is_some() || crate::modules::rust_analyzer().is_some(),
        &downloaded_tags(),
        fetch_release_tags(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choice_survives_a_restart_and_damage_means_automatic() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("nested/choice.json");
        assert_eq!(load(&file), ServerChoice::Automatic);
        let release = ServerChoice::Release("2025-12-08".to_owned());
        save(&file, &release).expect("saves");
        assert_eq!(load(&file), release);
        std::fs::write(&file, "{not json").expect("writes");
        assert_eq!(load(&file), ServerChoice::Automatic);
    }

    #[test]
    fn only_dated_releases_are_offered() {
        let body = br#"[{"tag_name":"nightly"},{"tag_name":"2026-10-26"},{"name":"x"}]"#;
        assert_eq!(parse_release_tags(body), ["2026-10-26"]);
        assert!(parse_release_tags(b"{\"message\":\"rate limit\"}").is_empty());
        assert!(parse_release_tags(b"").is_empty());
    }

    #[test]
    fn options_list_the_fixed_ones_then_releases_newest_first() {
        let downloaded = BTreeSet::from(["2024-07-22".to_owned()]);
        let current = ServerChoice::Release("2024-07-22".to_owned());
        let options = build_options(
            &current,
            true,
            &downloaded,
            vec!["2025-12-08".to_owned(), "2026-10-26".to_owned()],
        );
        let labels: Vec<_> = options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Automatic: match the project's Rust",
                "rust-analyzer from PATH",
                "Built into ttywright",
                "2026-10-26",
                "2025-12-08",
                "2024-07-22 (downloaded)",
            ]
        );
        let marked: Vec<_> = options.iter().filter(|o| o.current).collect();
        assert_eq!(marked.len(), 1);
        assert_eq!(marked[0].choice, current);
    }

    #[test]
    fn with_no_network_and_no_bundle_the_basics_remain() {
        let options = build_options(
            &ServerChoice::Automatic,
            false,
            &BTreeSet::new(),
            Vec::new(),
        );
        assert_eq!(options.len(), 2);
        assert!(options[0].current);
    }
}
