use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::time::SystemTime;

use image::{ImageReader, Limits};

use crate::core::image_art::{
    AsciiImage, HEAD_BYTES, ImageRequest, SAMPLES_X, SAMPLES_Y, cells_from_rgba, fit, is_image,
    is_remote,
};

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SIDE: u32 = 16_384;
const MAX_PIXELS: u64 = 64_000_000;
const MAX_DECODER_BYTES: u64 = 512 * 1024 * 1024;
const KEPT: usize = 32;

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        if let Some(byte) = hex {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn resolve(root: &Path, request: &ImageRequest) -> Result<PathBuf, String> {
    if is_remote(&request.source) {
        return Err("not a local file".to_owned());
    }
    let wanted = request.dir.join(percent_decoded(&request.source));
    let real = wanted
        .canonicalize()
        .map_err(|_| "file not found".to_owned())?;
    let inside = |place: &Path| place.canonicalize().is_ok_and(|p| real.starts_with(p));
    if inside(root) || inside(&request.dir) {
        Ok(real)
    } else {
        Err("outside the project".to_owned())
    }
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODER_BYTES);
    limits
}

fn reader(bytes: &[u8]) -> Result<ImageReader<std::io::Cursor<&[u8]>>, String> {
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(limits());
    Ok(reader)
}

fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("too large to show".to_owned());
    }
    Ok(bytes)
}

fn plain(error: &image::ImageError) -> String {
    match error {
        image::ImageError::Unsupported(_) => "not a picture this can show".to_owned(),
        image::ImageError::Limits(_) => "too large to show".to_owned(),
        _ => "the file is damaged".to_owned(),
    }
}

pub fn decode(path: &Path, cols: u16, rows: u16) -> Result<AsciiImage, String> {
    let bytes = read_capped(path)?;
    let (width, height) = reader(&bytes)?.into_dimensions().map_err(|e| plain(&e))?;
    if width > MAX_SIDE || height > MAX_SIDE || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("too large to show ({width}×{height})"));
    }
    let picture = reader(&bytes)?.decode().map_err(|e| plain(&e))?;
    let (c, r) = fit(width, height, cols, rows);
    let small = picture
        .resize_exact(
            u32::from(c) * SAMPLES_X,
            u32::from(r) * SAMPLES_Y,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    Ok(AsciiImage {
        cols: c,
        rows: r,
        cells: cells_from_rgba(small.as_raw(), c, r),
        width,
        height,
    })
}

pub fn open_outside(root: &Path, request: &ImageRequest) -> Result<(), String> {
    let path = resolve(root, request)?;
    let mut head = Vec::new();
    std::fs::File::open(&path)
        .and_then(|f| f.take(HEAD_BYTES).read_to_end(&mut head))
        .map_err(|e| e.to_string())?;
    if !is_image(&path, &head) {
        return Err("not a picture".to_owned());
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = std::process::Command::new(program)
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("{program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed"))
    }
}

type Key = (PathBuf, Option<SystemTime>, u64, u16, u16);

#[derive(Debug)]
pub struct Decoder {
    jobs: Sender<(PathBuf, ImageRequest)>,
}

impl Decoder {
    pub fn start(
        answer: impl Fn(ImageRequest, Result<AsciiImage, String>) + Send + 'static,
    ) -> Self {
        let (jobs, incoming) = channel::<(PathBuf, ImageRequest)>();
        std::thread::spawn(move || {
            let mut kept: HashMap<Key, Result<AsciiImage, String>> = HashMap::new();
            for (root, request) in incoming {
                let result = answered(&root, &request, &mut kept);
                answer(request, result);
            }
        });
        Self { jobs }
    }

    pub fn ask(&self, root: PathBuf, request: ImageRequest) {
        let _ = self.jobs.send((root, request));
    }
}

fn answered(
    root: &Path,
    request: &ImageRequest,
    kept: &mut HashMap<Key, Result<AsciiImage, String>>,
) -> Result<AsciiImage, String> {
    let path = resolve(root, request)?;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let key = (
        path.clone(),
        meta.modified().ok(),
        meta.len(),
        request.cols,
        request.rows,
    );
    if let Some(known) = kept.get(&key) {
        return known.clone();
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err("too large to show".to_owned());
    }
    let (cols, rows) = (request.cols, request.rows);
    let result = std::panic::catch_unwind(|| decode(&path, cols, rows))
        .unwrap_or_else(|_| Err("the file is damaged".to_owned()));
    if kept.len() >= KEPT {
        kept.clear();
    }
    kept.insert(key, result.clone());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::image_art::colored;
    use image::{GrayImage, Rgb, RgbImage};

    fn half_white(path: &Path) {
        let picture = RgbImage::from_fn(40, 20, |x, _| Rgb([if x < 20 { 255 } else { 0 }; 3]));
        picture.save(path).unwrap();
    }

    fn request(dir: &Path, source: &str) -> ImageRequest {
        ImageRequest {
            dir: dir.to_path_buf(),
            source: source.to_owned(),
            cols: 20,
            rows: 20,
        }
    }

    #[test]
    fn every_format_decodes_to_the_picture() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.png", "a.jpg", "a.gif", "a.bmp", "a.webp"] {
            let path = dir.path().join(name);
            half_white(&path);
            let art = decode(&path, 20, 20).unwrap();
            assert_eq!(
                (art.cols, art.rows, art.width, art.height),
                (20, 5, 40, 20),
                "{name}"
            );
            let row = art.row(2, 0, 20);
            let light = |c: &crate::core::image_art::ArtCell| {
                c.paper.is_some_and(|p| p[0] > 200) && colored(*c).ch == ' '
            };
            let dark = |c: &crate::core::image_art::ArtCell| {
                c.paper.is_some_and(|p| p[0] < 50) && colored(*c).ch == ' '
            };
            assert!(row[..8].iter().all(light), "{name}: {row:?}");
            assert!(row[12..].iter().all(dark), "{name}: {row:?}");
        }
    }

    #[test]
    fn a_header_that_promises_too_much_is_refused_before_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(60_000u32.to_be_bytes());
        png.extend(60_000u32.to_be_bytes());
        png.extend([8, 0, 0, 0, 0]);
        png.extend(crc_of(&png[12..]).to_be_bytes());
        let path = dir.path().join("bomb.png");
        std::fs::write(&path, png).unwrap();
        let reason = decode(&path, 20, 20).unwrap_err();
        assert!(reason.contains("too large"), "{reason}");
    }

    fn crc_of(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn a_damaged_or_oversized_file_gives_a_reason_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let damaged = dir.path().join("a.png");
        std::fs::write(&damaged, b"\x89PNG\r\n\x1a\ngarbage").unwrap();
        assert!(decode(&damaged, 20, 20).is_err());
        let text = dir.path().join("b.png");
        std::fs::write(&text, b"not a picture").unwrap();
        assert!(decode(&text, 20, 20).is_err());
        let big = dir.path().join("c.png");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert_eq!(decode(&big, 20, 20).unwrap_err(), "too large to show");
    }

    #[test]
    fn only_files_of_the_project_or_under_the_document_are_read() {
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.png");
        half_white(&secret);
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let docs = root.join("docs");
        std::fs::create_dir(&docs).unwrap();
        half_white(&root.join("up.png"));
        half_white(&root.join("a b.png"));

        assert!(
            resolve(&root, &request(&docs, "../up.png")).is_ok(),
            "in the project"
        );
        assert!(
            resolve(&root, &request(&docs, "../a%20b.png")).is_ok(),
            "escaped"
        );
        let absolute = secret.display().to_string();
        assert_eq!(
            resolve(&root, &request(&docs, &absolute)).unwrap_err(),
            "outside the project"
        );
        std::os::unix::fs::symlink(&secret, root.join("link.png")).unwrap();
        assert_eq!(
            resolve(&root, &request(&root, "link.png")).unwrap_err(),
            "outside the project",
            "a link is where it leads"
        );
        assert_eq!(
            resolve(&root, &request(&root, "https://example.com/a.png")).unwrap_err(),
            "not a local file"
        );
        assert_eq!(
            resolve(&root, &request(&root, "none.png")).unwrap_err(),
            "file not found"
        );
    }

    #[test]
    fn only_a_real_picture_ever_reaches_the_system_opener() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("evil.command"), "#!/bin/sh\n").unwrap();
        std::fs::write(root.join("fake.png"), "#!/bin/sh\n").unwrap();
        for source in ["evil.command", "fake.png"] {
            assert_eq!(
                open_outside(&root, &request(&root, source)).unwrap_err(),
                "not a picture",
                "{source}"
            );
        }
        assert_eq!(
            open_outside(&root, &request(&root, "none.png")).unwrap_err(),
            "file not found"
        );
    }

    #[test]
    fn a_changed_file_is_decoded_again_and_an_unchanged_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("a.png");
        half_white(&path);
        let mut kept = HashMap::new();
        let ask = request(&root, "a.png");
        assert!(answered(&root, &ask, &mut kept).is_ok());
        assert_eq!(kept.len(), 1);
        assert!(answered(&root, &ask, &mut kept).is_ok());
        assert_eq!(kept.len(), 1, "from what is kept");
        GrayImage::new(10, 10).save(&path).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        let art = answered(&root, &ask, &mut kept).unwrap();
        assert_eq!(art.width, 10, "the new picture");
    }
}
