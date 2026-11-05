use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use super::{Events, emit_state, server_runs};
use crate::core::LspState;

const TOOL_TIMEOUT: Duration = Duration::from_secs(600);
const RELEASES: &str = "https://github.com/rust-lang/rust-analyzer/releases/download";
const WEEKS_FORWARD: i64 = 4;
const WEEKS_BACK: i64 = 4;

#[derive(Debug, Clone)]
pub struct Download {
    pub dir: PathBuf,
    pub rustc: String,
    pub rustup: String,
    pub curl: String,
    pub gunzip: String,
    pub releases: String,
    pub pinned: Option<String>,
    pub host: String,
}

impl Download {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            rustc: "rustc".to_owned(),
            rustup: "rustup".to_owned(),
            curl: "curl".to_owned(),
            gunzip: "gunzip".to_owned(),
            releases: RELEASES.to_owned(),
            pinned: None,
            host: env!("BUILD_TARGET").to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Toolchain {
    host: String,
    built: i64,
}

fn parse_toolchain(version: &str) -> Option<Toolchain> {
    let field = |name: &str| {
        version
            .lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix(": "))
            .map(str::trim)
    };
    let mut date = field("commit-date")?.splitn(3, '-');
    let year = date.next()?.parse().ok()?;
    let month = date.next()?.parse().ok()?;
    let day = date.next()?.parse().ok()?;
    Some(Toolchain {
        host: field("host")?.to_owned(),
        built: days_from_civil(year, month, day),
    })
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn release_tags(built: i64) -> Vec<String> {
    let monday = built + (4 - built).rem_euclid(7);
    let forward = (0..WEEKS_FORWARD).map(|week| monday + week * 7);
    let back = (1..=WEEKS_BACK).map(|week| monday - week * 7);
    forward
        .chain(back)
        .map(|days| {
            let (year, month, day) = civil_from_days(days);
            format!("{year:04}-{month:02}-{day:02}")
        })
        .collect()
}

fn quiet(program: &str, root: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn succeeds(mut command: Command) -> bool {
    matches!(
        tokio::time::timeout(TOOL_TIMEOUT, command.status()).await,
        Ok(Ok(status)) if status.success()
    )
}

async fn project_toolchain(download: &Download, root: &Path) -> Option<Toolchain> {
    let output = tokio::time::timeout(
        TOOL_TIMEOUT,
        quiet(&download.rustc, root)
            .arg("-vV")
            .stdout(Stdio::piped())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_toolchain(&String::from_utf8_lossy(&output.stdout))
}

fn downloaded_path(download: &Download, tag: &str) -> PathBuf {
    download.dir.join(tag).join("rust-analyzer")
}

async fn already_downloaded(
    download: &Download,
    toolchain: &Toolchain,
    root: &Path,
) -> Option<String> {
    for tag in release_tags(toolchain.built) {
        let path = downloaded_path(download, &tag);
        if path.is_file() {
            let command = path.to_string_lossy().into_owned();
            if server_runs(&command, root).await {
                return Some(command);
            }
        }
    }
    None
}

async fn install_with_rustup(download: &Download, root: &Path) -> bool {
    let mut add = quiet(&download.rustup, root);
    add.args(["component", "add", "rust-analyzer", "rust-src"]);
    succeeds(add).await
}

async fn fetch_release(download: &Download, tag: &str, host: &str, root: &Path) -> Option<String> {
    let path = downloaded_path(download, tag);
    let dir = path.parent()?;
    tokio::fs::create_dir_all(dir).await.ok()?;
    let temporary = dir.join(format!(".download-{}", std::process::id()));
    let packed = temporary.with_extension("gz");
    let url = format!("{}/{tag}/rust-analyzer-{host}.gz", download.releases);
    let mut curl = quiet(&download.curl, root);
    curl.args(["--fail", "--location", "--silent", "--show-error"])
        .arg("--output")
        .arg(&packed)
        .arg(url);
    let mut gunzip = quiet(&download.gunzip, root);
    gunzip.arg("--force").arg(&packed);
    let unpacked = succeeds(curl).await && succeeds(gunzip).await;
    let command = if unpacked {
        finish_download(&temporary, &path, root).await
    } else {
        None
    };
    let _ = tokio::fs::remove_file(&packed).await;
    let _ = tokio::fs::remove_file(&temporary).await;
    if command.is_none() {
        let _ = tokio::fs::remove_dir(dir).await;
    }
    command
}

async fn finish_download(temporary: &Path, path: &Path, root: &Path) -> Option<String> {
    make_executable(temporary).await.ok()?;
    if !server_runs(&temporary.to_string_lossy(), root).await {
        return None;
    }
    tokio::fs::rename(temporary, path).await.ok()?;
    Some(path.to_string_lossy().into_owned())
}

#[cfg(unix)]
async fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).await
}

#[cfg(not(unix))]
async fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

async fn fetch_pinned(
    download: &Download,
    tag: &str,
    root: &Path,
    events: &Events,
) -> Option<String> {
    let path = downloaded_path(download, tag);
    let command = path.to_string_lossy().into_owned();
    if path.is_file() && server_runs(&command, root).await {
        return Some(command);
    }
    emit_state(events, LspState::Installing);
    fetch_release(download, tag, &download.host, root).await
}

pub(super) async fn fetch_server(
    download: &Download,
    command: &str,
    root: &Path,
    events: &Events,
) -> Option<String> {
    if let Some(tag) = &download.pinned {
        return fetch_pinned(download, tag, root, events).await;
    }
    let toolchain = project_toolchain(download, root).await?;
    if let Some(found) = already_downloaded(download, &toolchain, root).await {
        return Some(found);
    }
    emit_state(events, LspState::Installing);
    if install_with_rustup(download, root).await && server_runs(command, root).await {
        return Some(command.to_owned());
    }
    for tag in release_tags(toolchain.built) {
        if let Some(found) = fetch_release(download, &tag, &toolchain.host, root).await {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        for days in [-1, 0, 59, 20_000, 20_430, 40_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn tags_start_at_the_first_monday_on_or_after_the_build() {
        let on_a_monday = release_tags(days_from_civil(2025, 12, 8));
        assert_eq!(on_a_monday[0], "2025-12-08", "rustc 1.92");
        assert_eq!(on_a_monday[1], "2025-12-15");
        let on_a_thursday = release_tags(days_from_civil(2025, 12, 11));
        assert_eq!(on_a_thursday[0], "2025-12-15");
        let over_new_year = release_tags(days_from_civil(2025, 12, 30));
        assert_eq!(over_new_year[0], "2026-01-05");
    }

    #[test]
    fn tags_also_go_back_for_a_nightly_newer_than_any_release() {
        let tags = release_tags(days_from_civil(2025, 12, 8));
        assert_eq!(tags.len(), 8);
        assert!(tags.contains(&"2025-12-01".to_owned()));
    }

    #[test]
    fn the_toolchain_is_read_from_rustc() {
        let output = "rustc 1.92.0 (ded5c06cf 2025-12-08)\nbinary: rustc\n\
                      commit-date: 2025-12-08\nhost: aarch64-apple-darwin\n";
        assert_eq!(
            parse_toolchain(output),
            Some(Toolchain {
                host: "aarch64-apple-darwin".to_owned(),
                built: days_from_civil(2025, 12, 8),
            })
        );
        assert_eq!(
            parse_toolchain("rustc 1.0\ncommit-date: unknown\nhost: x"),
            None
        );
        assert_eq!(parse_toolchain(""), None);
    }
}
