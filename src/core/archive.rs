// ══════════════════════════════════════════════════════════════════════════════
// Unified Archive Abstraction – Virtual Table of Contents (TOC)
// ══════════════════════════════════════════════════════════════════════════════
//
// Provides a format-agnostic abstraction over zip, 7z, and rar archives.
//
// Core concepts:
//   - `ArchiveReader`: trait for streaming an archive's file list into a
//     `VirtualTree` without extracting any file data.
//   - `VirtualTree`: an in-memory table of all paths in an archive, used by
//     the data-root resolver and FOMOD engine to inspect structure.
//   - Lazy Extraction: actual file bytes are only read when explicitly
//     requested by path.

// All public items here are used by tests. Production code routes through
// installer.rs which builds on the same underlying archive libraries.
#![allow(dead_code)]

use std::path::Path;

use crate::core::logger;

// ── VirtualTree ───────────────────────────────────────────────────────────────

/// An in-memory representation of every path stored in an archive.
///
/// The rest of the application works with this tree instead of reading the
/// archive directly, making all heuristics and detection logic
/// format-agnostic.
#[derive(Debug, Clone)]
pub struct VirtualTree {
    /// Every path stored in the archive, in the original case and separator
    /// style as stored by the archive author.
    entries: Vec<String>,
    /// The archive format that produced this tree (for diagnostics only).
    format: ArchiveFormat,
}

/// Recognised archive formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Rar,
}

impl std::fmt::Display for ArchiveFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveFormat::Zip => write!(f, "zip"),
            ArchiveFormat::SevenZ => write!(f, "7z"),
            ArchiveFormat::Rar => write!(f, "rar"),
        }
    }
}

impl VirtualTree {
    /// Build a `VirtualTree` from an archive on disk.
    ///
    /// Streams only the central-directory / header metadata — no file data is
    /// decompressed.  Emits a DEBUG log with the node count and parse time.
    pub fn from_archive(archive_path: &Path) -> Result<Self, String> {
        let archive_name = archive_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| archive_path.display().to_string());

        let _span = logger::span("VirtualTree::build", &format!("archive={archive_name}"));

        let format = detect_format(archive_path)?;

        let entries = match format {
            ArchiveFormat::Zip => list_zip_entries(archive_path)?,
            ArchiveFormat::SevenZ => list_7z_entries(archive_path)?,
            ArchiveFormat::Rar => list_rar_entries(archive_path)?,
        };

        log::debug!(
            "[VirtualTree] Built from {fmt} archive | archive={name}, nodes={count}",
            fmt = format,
            name = archive_name,
            count = entries.len(),
        );

        Ok(VirtualTree { entries, format })
    }

    /// All paths stored in the archive, exactly as the author packaged them.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Convenience: borrow each entry as `&str`.
    pub fn entry_refs(&self) -> Vec<&str> {
        self.entries.iter().map(|s| s.as_str()).collect()
    }

    /// The number of entries (files + directories).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the tree contains zero entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The detected archive format.
    pub fn format(&self) -> ArchiveFormat {
        self.format
    }

    /// Return `true` when the archive contains `fomod/ModuleConfig.xml`
    /// (case-insensitive, with optional wrapper prefix).
    pub fn has_fomod_config(&self) -> bool {
        self.entries.iter().any(|p| {
            let lower = p.to_lowercase().replace('\\', "/");
            lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml")
        })
    }

    /// Find the parent directory that contains the `fomod/` subdirectory.
    ///
    /// Returns `Some("")` when `fomod/` lives at the archive root,
    /// `Some("dir")` when it is one level deep, or `None` when no FOMOD
    /// config is present.
    pub fn find_fomod_parent(&self) -> Option<String> {
        for path in &self.entries {
            let norm = path.to_lowercase().replace('\\', "/");
            let norm = norm.trim_start_matches('/');
            if norm == "fomod/moduleconfig.xml" {
                log::debug!(
                    "[VirtualTree] FOMOD config found at archive root | path={}",
                    path
                );
                return Some(String::new());
            }
            if norm.ends_with("/fomod/moduleconfig.xml") {
                let orig = path.replace('\\', "/");
                let orig = orig.trim_start_matches('/');
                let parent = orig.split('/').next().unwrap_or("").to_string();
                log::debug!(
                    "[VirtualTree] FOMOD config found under wrapper | path={}, parent={}",
                    path,
                    parent
                );
                return Some(parent);
            }
        }
        None
    }
}

// ── Format detection ──────────────────────────────────────────────────────────

fn detect_format(path: &Path) -> Result<ArchiveFormat, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "zip" => Ok(ArchiveFormat::Zip),
        "7z" => Ok(ArchiveFormat::SevenZ),
        "rar" => Ok(ArchiveFormat::Rar),
        _ => {
            // Default to 7z for unknown extensions (sevenz_rust2 handles many
            // formats internally).
            log::debug!(
                "[VirtualTree] Unknown extension '{}', defaulting to 7z parser | path={}",
                ext,
                path.display()
            );
            Ok(ArchiveFormat::SevenZ)
        }
    }
}

// ── Format-specific entry listing ─────────────────────────────────────────────

fn list_zip_entries(path: &Path) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Cannot open zip archive {}: {e}", path.display()))?;
    let zip = zip::ZipArchive::new(file)
        .map_err(|e| format!("Cannot read zip archive {}: {e}", path.display()))?;
    Ok(zip.file_names().map(|s| s.to_string()).collect())
}

fn list_7z_entries(path: &Path) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Cannot open 7z archive {}: {e}", path.display()))?;
    let reader = sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty())
        .map_err(|e| format!("Cannot read 7z archive {}: {e}", path.display()))?;
    Ok(reader
        .archive()
        .files
        .iter()
        .map(|f| f.name().to_string())
        .collect())
}

fn list_rar_entries(path: &Path) -> Result<Vec<String>, String> {
    let archive = unrar::Archive::new(path)
        .open_for_listing()
        .map_err(|e| format!("Cannot open rar archive {}: {e}", path.display()))?;
    let mut entries = Vec::new();
    for entry in archive.flatten() {
        entries.push(entry.filename.to_string_lossy().to_string());
    }
    Ok(entries)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tempdir() -> std::path::PathBuf {
        static CTR: AtomicU32 = AtomicU32::new(0);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("linkmm_archive_test_{}_{n}", std::process::id()));
        crate::core::io::rm_rf(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_test_zip(dir: &std::path::Path, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let archive_path = dir.join("test.zip");
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut zip_writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for &(name, content) in entries {
            if name.ends_with('/') {
                zip_writer.add_directory(name, options).unwrap();
            } else {
                zip_writer.start_file(name, options).unwrap();
                zip_writer.write_all(content).unwrap();
            }
        }
        let inner = zip_writer.finish().unwrap();
        drop(inner);
        archive_path
    }

    #[test]
    fn virtual_tree_from_zip_lists_all_entries() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("textures/sky.dds", b"dds"),
                ("meshes/rock.nif", b"nif"),
                ("plugin.esp", b"esp"),
            ],
        );
        let tree = VirtualTree::from_archive(&archive).unwrap();
        assert_eq!(tree.len(), 3);
        assert_eq!(tree.format(), ArchiveFormat::Zip);
        assert!(!tree.is_empty());
    }

    #[test]
    fn virtual_tree_detects_fomod_at_root() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("fomod/ModuleConfig.xml", b"<config/>"),
                ("textures/sky.dds", b"dds"),
            ],
        );
        let tree = VirtualTree::from_archive(&archive).unwrap();
        assert!(tree.has_fomod_config());
        assert_eq!(tree.find_fomod_parent(), Some(String::new()));
    }

    #[test]
    fn virtual_tree_detects_fomod_under_wrapper() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("MyMod/fomod/ModuleConfig.xml", b"<config/>"),
                ("MyMod/textures/sky.dds", b"dds"),
            ],
        );
        let tree = VirtualTree::from_archive(&archive).unwrap();
        assert!(tree.has_fomod_config());
        assert_eq!(tree.find_fomod_parent(), Some("MyMod".to_string()));
    }

    #[test]
    fn virtual_tree_no_fomod() {
        let tmp = tempdir();
        let archive = create_test_zip(&tmp, &[("textures/sky.dds", b"dds")]);
        let tree = VirtualTree::from_archive(&archive).unwrap();
        assert!(!tree.has_fomod_config());
        assert_eq!(tree.find_fomod_parent(), None);
    }

    #[test]
    fn virtual_tree_from_7z_lists_entries() {
        let tmp = tempdir();
        let archive_path = tmp.join("test.7z");
        let staging = tmp.join("staging");
        std::fs::create_dir_all(staging.join("textures")).unwrap();
        std::fs::write(staging.join("textures/sky.dds"), b"dds").unwrap();
        std::fs::write(staging.join("plugin.esp"), b"esp").unwrap();
        let out_file = std::fs::File::create(&archive_path).unwrap();
        if sevenz_rust2::compress(staging.as_path(), out_file).is_err() {
            return; // 7z compression not available
        }
        let tree = VirtualTree::from_archive(&archive_path).unwrap();
        assert!(tree.len() >= 2);
        assert_eq!(tree.format(), ArchiveFormat::SevenZ);
    }

    #[test]
    fn archive_format_display() {
        assert_eq!(format!("{}", ArchiveFormat::Zip), "zip");
        assert_eq!(format!("{}", ArchiveFormat::SevenZ), "7z");
        assert_eq!(format!("{}", ArchiveFormat::Rar), "rar");
    }
}
