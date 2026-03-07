use std::io::Read;
use std::path::{Path, PathBuf};

pub const DOWNLOAD_CANCELLED_ERROR: &str = "Download cancelled";

/// Download a file from `url` to `dest_path`, reporting progress via `on_progress(downloaded, total)`.
///
/// The file is first written to a `.part` temporary path and renamed on
/// completion so that a partial download is never mistaken for a finished one.
pub fn download_file(
    url: &str,
    dest_path: &Path,
    mut on_progress: impl FnMut(u64, u64) -> bool,
) -> Result<PathBuf, String> {
    let response = ureq::get(url)
        .set("User-Agent", "Linkmm/0.1.0")
        .call()
        .map_err(|e| format!("Download request failed: {e}"))?;

    let total: u64 = response
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let part_path = dest_path.with_extension(format!(
        "{}.part",
        dest_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
    ));

    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create download directory: {e}"))?;
    }

    let mut out = std::fs::File::create(&part_path)
        .map_err(|e| format!("Failed to create download file: {e}"))?;

    let mut reader = response.into_reader();
    let mut buf = [0u8; 65_536];
    let mut downloaded: u64 = 0;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Download read error: {e}"))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buf[..n])
            .map_err(|e| format!("Download write error: {e}"))?;
        downloaded += n as u64;
        if !on_progress(downloaded, total) {
            drop(out);
            let _ = std::fs::remove_file(&part_path);
            return Err(DOWNLOAD_CANCELLED_ERROR.to_string());
        }
    }

    drop(out);

    std::fs::rename(&part_path, dest_path)
        .map_err(|e| format!("Failed to finalise download: {e}"))?;

    Ok(dest_path.to_path_buf())
}
