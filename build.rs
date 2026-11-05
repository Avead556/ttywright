use std::error::Error;
use std::path::{Path, PathBuf};
use std::{env, fs};

const SERVER: &str = "TTYWRIGHT_BUNDLED_RUST_ANALYZER";
const DOWNLOADS: &str = "target/bundle";
const VERSION: &str = "scripts/rust-analyzer-version";
const COMPRESSION_LEVEL: i32 = 19;

fn downloaded_server() -> Result<Option<PathBuf>, Box<dyn Error>> {
    if env::var("PROFILE")? != "release" {
        return Ok(None);
    }
    let version = fs::read_to_string(VERSION)?;
    let path = Path::new(DOWNLOADS)
        .join(version.trim())
        .join(format!("rust-analyzer-{}", env::var("TARGET")?));
    Ok(path.is_file().then_some(path))
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-env-changed={SERVER}");
    println!("cargo::rerun-if-changed={DOWNLOADS}");
    println!("cargo::rerun-if-changed={VERSION}");
    println!("cargo::rustc-check-cfg=cfg(bundled_lsp)");
    println!("cargo::rustc-env=BUILD_TARGET={}", env::var("TARGET")?);
    if env::var_os("CARGO_FEATURE_STATIC_MODULES").is_none() {
        return Ok(());
    }
    let named = env::var_os(SERVER).map(PathBuf::from);
    let Some(path) = named.or(downloaded_server()?) else {
        return Ok(());
    };
    println!("cargo::rerun-if-changed={}", path.display());
    let binary = fs::read(&path)?;
    let compressed = zstd::encode_all(binary.as_slice(), COMPRESSION_LEVEL)?;
    fs::write(
        Path::new(&env::var("OUT_DIR")?).join("rust-analyzer.zst"),
        compressed,
    )?;
    println!("cargo::rustc-cfg=bundled_lsp");
    Ok(())
}
