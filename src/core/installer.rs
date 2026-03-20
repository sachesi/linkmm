use std::io::{BufWriter, Read};
use std::path::Path;

use crate::core::games::Game;
use crate::core::mods::{Mod, ModDatabase, ModManager};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Write-buffer capacity used when extracting individual archive entries.  A
/// 256 KB buffer reduces the number of `write` syscalls for archives that
/// contain many small files.
const EXTRACT_BUFFER_SIZE: usize = 256 * 1024;

/// How often the zip extraction loop calls the progress `tick` callback.
/// At 50 ms the progress bar pulses at ~20 Hz, which looks smooth enough
/// without calling `tick` on every single zip entry.
const EXTRACTION_TICK_INTERVAL_MS: u64 = 50;

/// Well-known subdirectory names that are expected directly inside a game's
/// `Data/` folder.  Used by the scoring heuristic to identify which directory
/// in an archive corresponds to the game data root.
const KNOWN_DATA_SUBDIRS: &[&str] = &[
    "meshes", "textures", "scripts", "sound", "music", "shaders",
    "lodsettings", "seq", "interface", "skse", "f4se", "nvse", "obse",
    "fose", "mwse", "xnvse", "strings", "video", "facegen", "grass",
    "shadersfx", "terrain", "dialogueviews", "vis", "lightingtemplate",
    "distantlod", "lod", "trees", "fxmaster", "sky",
];

/// Plugin file extensions that strongly indicate a game data root.
const KNOWN_PLUGIN_EXTS: &[&str] = &["esp", "esm", "esl"];

// ── Install strategy ──────────────────────────────────────────────────────────

/// How a mod archive should be installed.
#[derive(Debug, Clone)]
pub enum InstallStrategy {
    /// Extract to a mod folder under a `Data/` subdirectory and symlink into
    /// `<game_root>/Data`.
    Data,
    /// FOMOD-guided installation.  The `Vec<FomodFile>` contains the resolved
    /// list of files to install based on user selections.
    Fomod(Vec<FomodFile>),
}

// ── FOMOD types ───────────────────────────────────────────────────────────────
const FOMOD_DIR_PREFIX: &str = "fomod/";

/// A single file/folder mapping inside a FOMOD config.
#[derive(Debug, Clone)]
pub struct FomodFile {
    pub source: String,
    pub destination: String,
    pub priority: i32,
}

/// Selection type for a FOMOD plugin group.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupType {
    SelectAtLeastOne,
    SelectAtMostOne,
    SelectExactlyOne,
    SelectAll,
    SelectAny,
}

/// A single selectable plugin inside a FOMOD group.
#[derive(Debug, Clone)]
pub struct FomodPlugin {
    pub name: String,
    pub description: Option<String>,
    pub image_path: Option<String>,
    pub files: Vec<FomodFile>,
    pub type_descriptor: PluginType,
    pub condition_flags: Vec<ConditionFlag>,
    pub dependencies: Option<PluginDependencies>,
}

/// Condition flag contributed by a selected plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionFlag {
    pub name: String,
    pub value: String,
}

/// A single flag dependency for plugin visibility.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagDependency {
    pub flag: String,
    pub value: String,
}

/// Logical operator used for plugin dependency checks.
#[derive(Debug, Clone, PartialEq)]
pub enum DependencyOperator {
    And,
    Or,
}

/// Plugin visibility dependencies declared in FOMOD.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginDependencies {
    pub operator: DependencyOperator,
    pub flags: Vec<FlagDependency>,
}

/// Default selection state of a FOMOD plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginType {
    Required,
    Optional,
    Recommended,
    NotUsable,
}

/// A group of plugins that the user must choose from.
#[derive(Debug, Clone)]
pub struct PluginGroup {
    pub name: String,
    pub group_type: GroupType,
    pub plugins: Vec<FomodPlugin>,
}

/// A single install step presented in the FOMOD wizard.
#[derive(Debug, Clone)]
pub struct InstallStep {
    pub name: String,
    pub visible: Option<PluginDependencies>,
    pub groups: Vec<PluginGroup>,
}

/// Conditional files selected by dependency flags under
/// `<conditionalFileInstalls>`.
#[derive(Debug, Clone)]
pub struct ConditionalFileInstall {
    pub dependencies: PluginDependencies,
    pub files: Vec<FomodFile>,
}

/// Parsed FOMOD configuration.
#[derive(Debug, Clone)]
pub struct FomodConfig {
    pub mod_name: Option<String>,
    pub required_files: Vec<FomodFile>,
    pub steps: Vec<InstallStep>,
    pub conditional_file_installs: Vec<ConditionalFileInstall>,
}

// ── Install root detection heuristics ────────────────────────────────────────

/// Score a path prefix as a candidate game Data-root directory.
///
/// Higher scores indicate a better match.  Scoring weights:
/// * **+20** if the directory itself is named `data` (case-insensitive)
/// * **+10** for each direct-child directory that matches a known game
///   data subdirectory (meshes, textures, scripts, …)
/// * **+15** for each direct-child file with a plugin extension
///   (.esp / .esm / .esl)
///
/// Only the first occurrence of each direct-child name is counted so that
/// duplicate paths do not inflate the score.
fn score_as_data_root(prefix: &str, paths: &[&str]) -> i32 {
    let mut score = 0i32;

    // +20 if the directory is itself named "data".
    let prefix_norm = prefix.to_lowercase().replace('\\', "/");
    let dir_name = prefix_norm.trim_end_matches('/').split('/').next_back().unwrap_or("");
    if dir_name == "data" {
        score += 20;
    }

    // Build the lowercase search prefix with a trailing "/" for child matching.
    let search_prefix: String = if prefix_norm.trim_end_matches('/').is_empty() {
        String::new()
    } else {
        format!("{}/", prefix_norm.trim_end_matches('/'))
    };

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for path in paths {
        let path_norm = path.to_lowercase().replace('\\', "/");
        let path_norm = path_norm.trim_start_matches('/');

        // Strip the candidate prefix.
        let rel = if !search_prefix.is_empty() {
            match path_norm.strip_prefix(&search_prefix) {
                Some(r) => r,
                None => continue,
            }
        } else {
            path_norm
        };

        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }

        // Get the first path component (immediate child of the candidate dir).
        let first = rel.split('/').next().unwrap_or("").trim_end_matches('\\');
        if first.is_empty() || seen.contains(first) {
            continue;
        }
        seen.insert(first.to_string());

        // Known game data subdirectory.
        if KNOWN_DATA_SUBDIRS.contains(&first) {
            score += 10;
        }

        // Plugin file extension.
        if first.contains('.') {
            let ext = first.rsplit('.').next().unwrap_or("");
            if KNOWN_PLUGIN_EXTS.contains(&ext) {
                score += 15;
            }
        }
    }

    score
}

/// Scan all paths in an archive and return the prefix (with trailing `/`) that
/// best corresponds to the game's `Data/` directory.
///
/// The return value is the prefix to **strip** from archive entry names before
/// placing extracted files into `mod_dir/Data/`.  An empty string means the
/// archive root is already the data root (content goes directly into
/// `mod_dir/Data/`).
///
/// # Algorithm
/// Every ancestor directory of every path is scored with [`score_as_data_root`].
/// The directory with the highest score becomes the install root.  Ties are
/// broken in favour of the shallowest (shortest) directory to avoid over-
/// stripping.  If all scores are zero the empty string (archive root) is
/// returned and the caller should apply a secondary heuristic.
pub fn find_data_root_in_paths(paths: &[&str]) -> String {
    // Collect all unique ancestor directories (candidates).  The archive root
    // ("")  is always included.  Limit depth to 6 levels to avoid O(n²) work
    // on pathological archives with very deep nesting.
    const MAX_DEPTH: usize = 6;

    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    dirs.insert(String::new());

    for path in paths {
        let p = path.replace('\\', "/");
        let p = p.trim_start_matches('/');
        let mut current = String::new();
        let mut depth = 0usize;
        for component in p.split('/') {
            if component.is_empty() {
                continue;
            }
            depth += 1;
            if depth > MAX_DEPTH {
                break;
            }
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(component);
            dirs.insert(current.clone());
        }
    }

    let mut best_dir = String::new();
    let mut best_score = -1i32;

    for dir in &dirs {
        let score = score_as_data_root(dir, paths);
        // Prefer a higher score; on a tie keep the shallower (shorter) path.
        if score > best_score || (score == best_score && dir.len() < best_dir.len()) {
            best_score = score;
            best_dir = dir.clone();
        }
    }

    if best_dir.is_empty() {
        String::new()
    } else {
        format!("{best_dir}/")
    }
}

/// Compute the common single-level top-directory prefix shared by all entries
/// in a path list.
///
/// Returns a string with a trailing `/` if all entries share the same
/// top-level folder, or an empty string otherwise.  This is the path-slice
/// equivalent of [`find_common_prefix`].
fn find_common_prefix_from_paths(paths: &[&str]) -> String {
    if paths.is_empty() {
        return String::new();
    }

    let mut first_top: Option<String> = None;
    let mut all_same = true;

    for path in paths {
        let p = path.replace('\\', "/");
        let top = p.trim_start_matches('/').split('/').next().unwrap_or("");
        if top.is_empty() {
            continue;
        }
        match &first_top {
            None => first_top = Some(top.to_string()),
            Some(ft) if ft.as_str() != top => {
                all_same = false;
                break;
            }
            _ => {}
        }
    }

    if all_same {
        if let Some(ft) = first_top {
            if paths.len() > 1 {
                return format!("{ft}/");
            }
        }
    }
    String::new()
}

/// Return `true` if the paths (already stripped of any outer wrapper prefix)
/// look like game-**root**-level content rather than game-Data content.
///
/// Indicators checked:
/// * Known root-level directory names: `enbseries`, `reshade-shaders`,
///   `reshade`
/// * DLL / ASI files at the top level
///
/// This function intentionally has a low false-positive rate: it only triggers
/// on clear root-level markers so that ambiguous archives default to the safer
/// Data/ install path.
fn looks_like_root_level_mod(paths: &[&str]) -> bool {
    const ROOT_DIRS: &[&str] = &["enbseries", "reshade-shaders", "reshade"];
    const ROOT_EXTS: &[&str] = &["dll", "asi"];

    for path in paths {
        let p = path.to_lowercase().replace('\\', "/");
        let first = p.trim_start_matches('/').split('/').next().unwrap_or("");
        if first.is_empty() {
            continue;
        }
        if ROOT_DIRS.contains(&first) {
            return true;
        }
        if first.contains('.') {
            let ext = first.rsplit('.').next().unwrap_or("");
            if ROOT_EXTS.contains(&ext) {
                return true;
            }
        }
    }
    false
}

/// Return `true` if the archive follows BAIN (Wrye Bash) package conventions.
///
/// BAIN archives have **two or more** top-level directories whose names start
/// with exactly two ASCII digits followed by a space or underscore
/// (`"00 Core/"`, `"01 Optional/"`, `"02_Patches/"`, …).
pub fn is_bain_archive(paths: &[&str]) -> bool {
    let mut top_dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for path in paths {
        let p = path.replace('\\', "/");
        let p = p.trim_start_matches('/').to_string();
        if let Some(first) = p.split('/').next() {
            if !first.is_empty() {
                top_dirs.insert(first.to_lowercase());
            }
        }
    }

    if top_dirs.len() < 2 {
        return false;
    }

    // Every top-level entry must match the BAIN "NN " or "NN_" pattern.
    top_dirs.iter().all(|d| {
        d.len() >= 3
            && d.chars().take(2).all(|c| c.is_ascii_digit())
            && matches!(d.chars().nth(2), Some(' ') | Some('_'))
    })
}

/// Return the sorted list of BAIN package directory names from a path list.
///
/// The names are returned in ascending order so that higher-numbered packages
/// are installed last and naturally override earlier ones.
fn collect_bain_top_dirs(paths: &[&str]) -> Vec<String> {
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for path in paths {
        let p = path.replace('\\', "/");
        let p_lower = p.trim_start_matches('/').to_lowercase();
        if let Some(first) = p_lower.split('/').next() {
            if !first.is_empty()
                && first.len() >= 3
                && first.chars().take(2).all(|c| c.is_ascii_digit())
                && matches!(first.chars().nth(2), Some(' ') | Some('_'))
            {
                // Use the original case for the directory name so that the
                // file system path matches the archive entry.
                let orig_p = p.trim_start_matches('/');
                if let Some(orig_first) = orig_p.split('/').next() {
                    dirs.insert(orig_first.to_string());
                }
            }
        }
    }
    dirs.into_iter().collect()
}

// ── Strategy detection ────────────────────────────────────────────────────────

/// Examine the contents of a zip archive and determine the best install
/// strategy.
///
/// - If a `fomod/ModuleConfig.xml` is found → `Fomod` (with empty file list;
///   the caller must run the wizard to populate it).
/// - Otherwise → `Data` (all content is placed under a `Data/` subdirectory
///   inside the mod folder and symlinked into `<game_root>/Data`).
pub fn detect_strategy(archive_path: &Path) -> Result<InstallStrategy, String> {
    if !archive_path.exists() {
        return Err(format!("Cannot open archive: {}", archive_path.display()));
    }
    if !has_zip_extension(archive_path) {
        // Use fast header listing (no extraction) to detect FOMOD presence.
        // If listing fails (e.g. 7z not installed or corrupt archive), fall
        // back gracefully to Data strategy.
        let entries = list_archive_entries_with_7z(archive_path).unwrap_or_default();
        let has_fomod = entries.iter().any(|p| {
            let lower = p.to_lowercase().replace('\\', "/");
            lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml")
        });
        // Return without parsing the full FOMOD XML here — the caller should
        // call parse_fomod_from_archive separately when the full config is
        // needed (e.g. to show the wizard).  This avoids a redundant 7z
        // subprocess invocation inside detect_strategy.
        return Ok(if has_fomod {
            InstallStrategy::Fomod(vec![])
        } else {
            InstallStrategy::Data
        });
    }

    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    let mut has_fomod = false;

    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;
        let name_lower = entry.name().to_lowercase();

        if name_lower == "fomod/moduleconfig.xml" || name_lower.ends_with("/fomod/moduleconfig.xml")
        {
            has_fomod = true;
        }
    }

    if has_fomod {
        let config = parse_fomod_from_zip(archive_path)?;
        // Return Fomod with empty file list – the caller will run the wizard
        Ok(InstallStrategy::Fomod(config.required_files.clone()))
    } else {
        Ok(InstallStrategy::Data)
    }
}

fn top_level_component(path: &str) -> &str {
    let stripped = path.strip_prefix('/').unwrap_or(path);
    stripped.split('/').next().unwrap_or("")
}

/// Return `true` when the archive already contains a `Data/` folder after the
/// common wrapper prefix is stripped.
///
/// Examples:
/// - `Data/textures/sky.dds` → common prefix `Data/` stripped → bare files →
///   returns `false` (content needs to go into `Data/`).
/// - `SomeMod/Data/textures/sky.dds` → common prefix `SomeMod/` stripped →
///   remaining starts with `Data/` → returns `true`.
/// - `textures/sky.dds` → no prefix → returns `false`.
fn archive_has_data_folder(archive_path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(archive_path) else {
        return false;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return false;
    };
    // find_common_prefix now uses file_names() (&self), so we can reuse the
    // same ZipArchive instance for the entry scan below.
    let prefix = find_common_prefix(&zip);

    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else {
            continue;
        };
        let name = entry.name();
        let rel = if !prefix.is_empty() {
            name.strip_prefix(&prefix).unwrap_or(name)
        } else {
            name
        };
        let rel_lower = rel.to_lowercase();
        let top = top_level_component(&rel_lower);
        if top == "data" {
            return true;
        }
    }
    false
}

// ── FOMOD XML parser ──────────────────────────────────────────────────────────

/// Parse a FOMOD `ModuleConfig.xml` from a supported archive.
pub fn parse_fomod_from_archive(archive_path: &Path) -> Result<FomodConfig, String> {
    if has_zip_extension(archive_path) {
        return parse_fomod_from_zip(archive_path);
    }

    // For non-zip archives: list entries to find the FOMOD XML path, then
    // extract only that single file.  This is much faster than a full
    // extraction for large archives.
    let entries = list_archive_entries_with_7z(archive_path)?;
    let fomod_entry = entries
        .iter()
        .find(|p| {
            let lower = p.to_lowercase().replace('\\', "/");
            lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml")
        })
        .ok_or_else(|| "No fomod/ModuleConfig.xml found in archive".to_string())?;

    let tmp_extract = create_temp_extract_dir()?;
    let result = (|| {
        extract_single_file_with_7z(archive_path, fomod_entry, &tmp_extract)?;
        let config_path = find_fomod_config_in_dir(&tmp_extract)
            .ok_or_else(|| "fomod/ModuleConfig.xml not found after extraction".to_string())?;
        let xml_bytes = std::fs::read(&config_path)
            .map_err(|e| format!("Failed to read fomod config {}: {e}", config_path.display()))?;
        parse_fomod_xml(&xml_bytes)
    })();

    if let Err(e) = std::fs::remove_dir_all(&tmp_extract) {
        log::warn!(
            "Failed to remove temporary extraction directory {}: {e}",
            tmp_extract.display()
        );
    }

    result
}

/// Parse a FOMOD `ModuleConfig.xml` from inside a zip archive.
pub fn parse_fomod_from_zip(archive_path: &Path) -> Result<FomodConfig, String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    // Find the fomod config file (case-insensitive)
    let fomod_entry_name = find_fomod_entry(&mut zip)?;

    let mut entry = zip
        .by_name(&fomod_entry_name)
        .map_err(|e| format!("Cannot read fomod config: {e}"))?;
    let mut xml_bytes = Vec::new();
    entry
        .read_to_end(&mut xml_bytes)
        .map_err(|e| format!("Failed to read fomod config: {e}"))?;

    parse_fomod_xml(&xml_bytes)
}

fn find_fomod_config_in_dir(root: &Path) -> Option<std::path::PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                log::warn!("Failed to read extracted archive directory {}: {e}", dir.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_lower = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                .collect::<Vec<_>>()
                .join("/");
            if rel_lower.ends_with("fomod/moduleconfig.xml") {
                return Some(path);
            }
        }
    }
    None
}

fn find_fomod_entry(zip: &mut zip::ZipArchive<std::fs::File>) -> Result<String, String> {
    for i in 0..zip.len() {
        let entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;
        let lower = entry.name().to_lowercase();
        if lower == "fomod/moduleconfig.xml" || lower.ends_with("/fomod/moduleconfig.xml") {
            return Ok(entry.name().to_string());
        }
    }
    Err("No fomod/ModuleConfig.xml found in archive".to_string())
}

/// Load a file from an archive by path, using case-insensitive matching and
/// common FOMOD-relative fallbacks.  Supports both zip archives and non-zip
/// archives (7z, rar, etc. — extracted via the `7z` command).
pub fn read_archive_file_bytes(
    archive_path: &Path,
    relative_path: &str,
) -> Result<Vec<u8>, String> {
    if !has_zip_extension(archive_path) {
        return read_archive_file_bytes_non_zip(archive_path, relative_path);
    }

    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    let target = normalise_path(relative_path);
    let target_lower = target.to_lowercase();
    if target_lower.is_empty() {
        return Err("Empty archive path".to_string());
    }
    let fomod_target = format!("{FOMOD_DIR_PREFIX}{target_lower}");

    // FOMOD image paths are often relative to `fomod/`, while some archives
    // store them at the root or inside a wrapped top-level folder. Match both
    // direct and `fomod/`-prefixed variants, including wrapped prefixes.
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name_norm = normalise_path(entry.name());
        let name_lower = name_norm.to_lowercase();
        let matches = name_lower == target_lower
            || name_lower.ends_with(&format!("/{target_lower}"))
            || name_lower == fomod_target
            || name_lower.ends_with(&format!("/{fomod_target}"));
        if matches {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("Failed reading archive file {name_norm}: {e}"))?;
            return Ok(bytes);
        }
    }

    Err(format!("Archive file not found: {relative_path}"))
}

/// Read a file from a non-zip archive (7z, rar, etc.) using the same
/// case-insensitive / `fomod/`-relative matching logic as the zip variant.
///
/// Instead of fully extracting the archive, this function first lists the
/// archive's contents to find the exact stored path, then extracts only that
/// single file.  This is orders of magnitude faster for large archives.
fn read_archive_file_bytes_non_zip(
    archive_path: &Path,
    relative_path: &str,
) -> Result<Vec<u8>, String> {
    // Build the list of archive entries (fast: reads only headers, no extraction).
    let entries = list_archive_entries_with_7z(archive_path)?;

    let target = normalise_path(relative_path);
    let target_lower = target.to_lowercase();
    if target_lower.is_empty() {
        return Err("Empty archive path".to_string());
    }
    let fomod_target = format!("{FOMOD_DIR_PREFIX}{target_lower}");

    // Find the matching entry using the same fallback aliases as the zip path.
    let matching_entry = entries.iter().find(|p| {
        let norm = normalise_path(p);
        let lower = norm.to_lowercase();
        lower == target_lower
            || lower.ends_with(&format!("/{target_lower}"))
            || lower == fomod_target
            || lower.ends_with(&format!("/{fomod_target}"))
    });

    let Some(entry_path) = matching_entry else {
        return Err(format!("Archive file not found: {relative_path}"));
    };

    // Extract only that single file.
    let tmp = create_temp_extract_dir()?;
    let result = (|| {
        extract_single_file_with_7z(archive_path, entry_path, &tmp)?;
        // The file was extracted preserving directory structure.  Locate it.
        let normalised = normalise_path(entry_path);
        let extracted_path = tmp.join(Path::new(&normalised));
        std::fs::read(&extracted_path)
            .map_err(|e| format!("Failed to read extracted file {}: {e}", extracted_path.display()))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Recursively collect all regular files under `dir`, returning tuples of
/// `(lowercase_relative_path, full_path)` relative to `root`.
fn collect_fs_files(
    root: &Path,
    dir: &Path,
    result: &mut Vec<(String, std::path::PathBuf)>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fs_files(root, &path, result);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = normalise_path(&rel.to_string_lossy());
                result.push((rel_str.to_lowercase(), path));
            }
        }
    }
}

/// Recursively collect all entries (files and directories) under `dir`,
/// returning tuples of `(lowercase_relative_path, original_relative_path)`
/// relative to `root`.
fn collect_fs_entries(
    root: &Path,
    dir: &Path,
    result: &mut Vec<(String, String)>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_str = normalise_path(&rel.to_string_lossy());
        if rel_str.is_empty() {
            continue;
        }
        result.push((rel_str.to_lowercase(), rel_str));
        if path.is_dir() {
            collect_fs_entries(root, &path, result);
        }
    }
}

/// Determine the common top-level prefix shared by all entries in a filesystem
/// entry map (mirrors [`find_common_prefix`] for zip archives).
///
/// Returns `(original_case_prefix, lowercase_prefix)`, both including a
/// trailing `/`, or two empty strings when no common prefix is found.
fn find_fs_common_prefix(entry_map: &[(String, String)]) -> (String, String) {
    if entry_map.len() <= 1 {
        return (String::new(), String::new());
    }
    let mut first_lower: Option<String> = None;
    let mut first_orig: Option<String> = None;
    let mut all_same = true;
    for (lower, orig) in entry_map {
        let tl = lower.split('/').next().unwrap_or("");
        let to = orig.split('/').next().unwrap_or("");
        if tl.is_empty() {
            continue;
        }
        match &first_lower {
            None => {
                first_lower = Some(tl.to_string());
                first_orig = Some(to.to_string());
            }
            Some(ft) if ft.as_str() != tl => {
                all_same = false;
                break;
            }
            _ => {}
        }
    }
    if all_same {
        if let (Some(tl), Some(to)) = (first_lower, first_orig) {
            return (format!("{to}/"), format!("{tl}/"));
        }
    }
    (String::new(), String::new())
}

/// Filter `entry_map` to entries whose lowercase relative path equals
/// `source_lower` or starts with `source_lower/`, returning the original
/// relative paths.
fn collect_matching_fs_entries(
    entry_map: &[(String, String)],
    source_lower: &str,
) -> Vec<String> {
    entry_map
        .iter()
        .filter(|(nl, _)| {
            *nl == source_lower
                || nl.starts_with(&format!("{source_lower}/"))
                || nl.starts_with(&format!("{source_lower}\\"))
        })
        .map(|(_, orig)| orig.clone())
        .collect()
}

fn parse_fomod_xml(xml_bytes: &[u8]) -> Result<FomodConfig, String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_reader(xml_bytes);
    reader.config_mut().trim_text(true);

    let mut config = FomodConfig {
        mod_name: None,
        required_files: Vec::new(),
        steps: Vec::new(),
        conditional_file_installs: Vec::new(),
    };

    let mut buf = Vec::new();
    let mut path_stack: Vec<String> = Vec::new();

    // Current parsing context
    let mut current_step: Option<InstallStep> = None;
    let mut current_group: Option<PluginGroup> = None;
    let mut current_plugin: Option<FomodPlugin> = None;
    let mut current_text = String::new();
    let mut current_condition_flag_name: Option<String> = None;
    let mut in_required = false;
    let mut in_visible = false;
    let mut in_pattern = false;
    let mut current_pattern_dependencies: Option<PluginDependencies> = None;
    let mut current_pattern_files: Vec<FomodFile> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                path_stack.push(tag.clone());

                match tag.as_str() {
                    "modulename" => {}
                    "requiredinstallfiles" => {
                        in_required = true;
                    }
                    "installstep" => {
                        let name = get_attr(e, "name").unwrap_or_default();
                        current_step = Some(InstallStep {
                            name,
                            visible: None,
                            groups: Vec::new(),
                        });
                    }
                    "visible" => {
                        in_visible = true;
                    }
                    "pattern" => {
                        in_pattern = true;
                        current_pattern_dependencies = None;
                        current_pattern_files.clear();
                    }
                    "group" => {
                        let name = get_attr(e, "name").unwrap_or_default();
                        let type_str =
                            get_attr(e, "type").unwrap_or_else(|| "SelectAny".to_string());
                        let group_type = match type_str.to_lowercase().as_str() {
                            "selectatleastone" => GroupType::SelectAtLeastOne,
                            "selectatmostone" => GroupType::SelectAtMostOne,
                            "selectexactlyone" => GroupType::SelectExactlyOne,
                            "selectall" => GroupType::SelectAll,
                            _ => GroupType::SelectAny,
                        };
                        current_group = Some(PluginGroup {
                            name,
                            group_type,
                            plugins: Vec::new(),
                        });
                    }
                    "plugin" => {
                        let name = get_attr(e, "name").unwrap_or_default();
                        current_plugin = Some(FomodPlugin {
                            name,
                            description: None,
                            image_path: None,
                            files: Vec::new(),
                            type_descriptor: PluginType::Optional,
                            condition_flags: Vec::new(),
                            dependencies: None,
                        });
                    }
                    "image" => {
                        if let Some(ref mut plugin) = current_plugin {
                            plugin.image_path = get_attr(e, "path");
                        }
                    }
                    "file" | "folder" => {
                        let source = get_attr(e, "source").unwrap_or_default();
                        let destination = get_attr(e, "destination").unwrap_or_default();
                        let priority = get_attr(e, "priority")
                            .and_then(|p| p.parse::<i32>().ok())
                            .unwrap_or(0);

                        let fomod_file = FomodFile {
                            source,
                            destination,
                            priority,
                        };

                        if in_pattern {
                            current_pattern_files.push(fomod_file);
                        } else if in_required && current_plugin.is_none() {
                            config.required_files.push(fomod_file);
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.files.push(fomod_file);
                        }
                    }
                    "type" => {
                        if current_plugin.is_some() {
                            let name = get_attr(e, "name").unwrap_or_default();
                            if let Some(ref mut plugin) = current_plugin {
                                plugin.type_descriptor = match name.to_lowercase().as_str() {
                                    "required" => PluginType::Required,
                                    "recommended" => PluginType::Recommended,
                                    "notusable" | "couldbeusable" => PluginType::NotUsable,
                                    _ => PluginType::Optional,
                                };
                            }
                        }
                    }
                    "dependencies" => {
                        let operator = match get_attr(e, "operator")
                            .unwrap_or_else(|| "And".to_string())
                            .to_lowercase()
                            .as_str()
                        {
                            "or" => DependencyOperator::Or,
                            _ => DependencyOperator::And,
                        };
                        let deps = PluginDependencies {
                            operator,
                            flags: Vec::new(),
                        };
                        if in_pattern {
                            current_pattern_dependencies = Some(deps);
                        } else if in_visible {
                            if let Some(ref mut step) = current_step {
                                step.visible = Some(deps);
                            }
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.dependencies = Some(deps);
                        }
                    }
                    "flagdependency" => {
                        let flag = get_attr(e, "flag").unwrap_or_default();
                        let value = get_attr(e, "value").unwrap_or_default();
                        if !flag.is_empty() {
                            if in_pattern {
                                if current_pattern_dependencies.is_none() {
                                    current_pattern_dependencies = Some(PluginDependencies {
                                        operator: DependencyOperator::And,
                                        flags: Vec::new(),
                                    });
                                }
                                if let Some(ref mut deps) = current_pattern_dependencies {
                                    deps.flags.push(FlagDependency { flag, value });
                                }
                            } else if in_visible {
                                if let Some(ref mut step) = current_step {
                                    if step.visible.is_none() {
                                        step.visible = Some(PluginDependencies {
                                            operator: DependencyOperator::And,
                                            flags: Vec::new(),
                                        });
                                    }
                                    if let Some(ref mut deps) = step.visible {
                                        deps.flags.push(FlagDependency { flag, value });
                                    }
                                }
                            } else if let Some(ref mut plugin) = current_plugin {
                                if let Some(ref mut deps) = plugin.dependencies {
                                    deps.flags.push(FlagDependency { flag, value });
                                }
                            }
                        }
                    }
                    "flag" => {
                        let in_condition_flags = path_stack.iter().any(|p| p == "conditionflags");
                        if in_condition_flags && current_plugin.is_some() {
                            current_condition_flag_name = get_attr(e, "name");
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                match tag.as_str() {
                    "file" | "folder" => {
                        let source = get_attr(e, "source").unwrap_or_default();
                        let destination = get_attr(e, "destination").unwrap_or_default();
                        let priority = get_attr(e, "priority")
                            .and_then(|p| p.parse::<i32>().ok())
                            .unwrap_or(0);

                        let fomod_file = FomodFile {
                            source,
                            destination,
                            priority,
                        };

                        if in_pattern {
                            current_pattern_files.push(fomod_file);
                        } else if in_required && current_plugin.is_none() {
                            config.required_files.push(fomod_file);
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.files.push(fomod_file);
                        }
                    }
                    "type" => {
                        if let Some(ref mut plugin) = current_plugin {
                            let name = get_attr(e, "name").unwrap_or_default();
                            plugin.type_descriptor = match name.to_lowercase().as_str() {
                                "required" => PluginType::Required,
                                "recommended" => PluginType::Recommended,
                                "notusable" | "couldbeusable" => PluginType::NotUsable,
                                _ => PluginType::Optional,
                            };
                        }
                    }
                    "dependencies" => {
                        let operator = match get_attr(e, "operator")
                            .unwrap_or_else(|| "And".to_string())
                            .to_lowercase()
                            .as_str()
                        {
                            "or" => DependencyOperator::Or,
                            _ => DependencyOperator::And,
                        };
                        let deps = PluginDependencies {
                            operator,
                            flags: Vec::new(),
                        };
                        if in_pattern {
                            current_pattern_dependencies = Some(deps);
                        } else if in_visible {
                            if let Some(ref mut step) = current_step {
                                step.visible = Some(deps);
                            }
                        } else if let Some(ref mut plugin) = current_plugin {
                            plugin.dependencies = Some(deps);
                        }
                    }
                    "image" => {
                        if let Some(ref mut plugin) = current_plugin {
                            plugin.image_path = get_attr(e, "path");
                        }
                    }
                    "flag" => {
                        let in_condition_flags = path_stack.iter().any(|p| p == "conditionflags");
                        if in_condition_flags {
                            if let Some(ref mut plugin) = current_plugin {
                                let name = get_attr(e, "name").unwrap_or_default();
                                let value = get_attr(e, "value").unwrap_or_default();
                                if !name.is_empty() && !value.is_empty() {
                                    plugin.condition_flags.push(ConditionFlag { name, value });
                                }
                            }
                        }
                    }
                    "flagdependency" => {
                        let flag = get_attr(e, "flag").unwrap_or_default();
                        let value = get_attr(e, "value").unwrap_or_default();
                        if !flag.is_empty() {
                            if in_pattern {
                                if current_pattern_dependencies.is_none() {
                                    current_pattern_dependencies = Some(PluginDependencies {
                                        operator: DependencyOperator::And,
                                        flags: Vec::new(),
                                    });
                                }
                                if let Some(ref mut deps) = current_pattern_dependencies {
                                    deps.flags.push(FlagDependency { flag, value });
                                }
                            } else if in_visible {
                                if let Some(ref mut step) = current_step {
                                    if step.visible.is_none() {
                                        step.visible = Some(PluginDependencies {
                                            operator: DependencyOperator::And,
                                            flags: Vec::new(),
                                        });
                                    }
                                    if let Some(ref mut deps) = step.visible {
                                        deps.flags.push(FlagDependency { flag, value });
                                    }
                                }
                            } else if let Some(ref mut plugin) = current_plugin {
                                if let Some(ref mut deps) = plugin.dependencies {
                                    deps.flags.push(FlagDependency { flag, value });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                current_text = e.unescape().unwrap_or_default().to_string();
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                path_stack.pop();

                match tag.as_str() {
                    "modulename" => {
                        if !current_text.is_empty() {
                            config.mod_name = Some(current_text.clone());
                        }
                    }
                    "requiredinstallfiles" => {
                        in_required = false;
                    }
                    "description" => {
                        if let Some(ref mut plugin) = current_plugin {
                            if !current_text.is_empty() {
                                plugin.description = Some(current_text.clone());
                            }
                        }
                    }
                    "flag" => {
                        let in_condition_flags = path_stack
                            .last()
                            .map(|p| p == "conditionflags")
                            .unwrap_or(false);
                        if in_condition_flags {
                            if let Some(name) = current_condition_flag_name.take() {
                                if let Some(ref mut plugin) = current_plugin {
                                    if !current_text.is_empty() {
                                        plugin.condition_flags.push(ConditionFlag {
                                            name,
                                            value: current_text.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    "dependencies" => {
                        if in_pattern {
                            if let Some(ref deps) = current_pattern_dependencies {
                                if deps.flags.is_empty() {
                                    current_pattern_dependencies = None;
                                }
                            }
                        } else if in_visible {
                            if let Some(ref mut step) = current_step {
                                if let Some(ref deps) = step.visible {
                                    if deps.flags.is_empty() {
                                        step.visible = None;
                                    }
                                }
                            }
                        } else if let Some(ref mut plugin) = current_plugin {
                            if let Some(ref deps) = plugin.dependencies {
                                if deps.flags.is_empty() {
                                    plugin.dependencies = None;
                                }
                            }
                        }
                    }
                    "visible" => {
                        in_visible = false;
                    }
                    "plugin" => {
                        if let Some(plugin) = current_plugin.take() {
                            if let Some(ref mut group) = current_group {
                                group.plugins.push(plugin);
                            }
                        }
                    }
                    "group" => {
                        if let Some(group) = current_group.take() {
                            if let Some(ref mut step) = current_step {
                                step.groups.push(group);
                            }
                        }
                    }
                    "installstep" => {
                        if let Some(step) = current_step.take() {
                            config.steps.push(step);
                        }
                    }
                    "pattern" => {
                        let dependencies = current_pattern_dependencies.take().unwrap_or(
                            PluginDependencies {
                                operator: DependencyOperator::And,
                                flags: Vec::new(),
                            },
                        );
                        if !current_pattern_files.is_empty() {
                            config
                                .conditional_file_installs
                                .push(ConditionalFileInstall {
                                    dependencies,
                                    files: std::mem::take(&mut current_pattern_files),
                                });
                        } else {
                            current_pattern_files.clear();
                        }
                        in_pattern = false;
                    }
                    _ => {}
                }
                current_text.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(format!("XML parse error: {e}"));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(config)
}

fn get_attr(event: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    for attr in event.attributes().flatten() {
        if attr.key.as_ref() == name.as_bytes() {
            return Some(attr.unescape_value().unwrap_or_default().to_string());
        }
    }
    None
}

// ── Installation logic ────────────────────────────────────────────────────────

/// Install a mod from a zip archive.
///
/// 1. Extracts the archive into `<mod_dir>/Data/` so that the managed directory
///    always uses the `{uuid}/Data/…` structure.
/// 2. Updates the mod database.
/// 3. Returns the created `Mod` entry.
pub fn install_mod_from_archive(
    archive_path: &Path,
    game: &Game,
    mod_name: &str,
    strategy: &InstallStrategy,
) -> Result<Mod, String> {
    install_mod_from_archive_with_nexus_ticking(archive_path, game, mod_name, strategy, None, &|| {})
}

pub fn install_mod_from_archive_with_nexus(
    archive_path: &Path,
    game: &Game,
    mod_name: &str,
    strategy: &InstallStrategy,
    nexus_id: Option<u32>,
) -> Result<Mod, String> {
    install_mod_from_archive_with_nexus_ticking(archive_path, game, mod_name, strategy, nexus_id, &|| {})
}

/// Like [`install_mod_from_archive_with_nexus`] but calls `tick()` periodically
/// during extraction (iterating over entries).
///
/// The `tick` function is invoked on the calling thread roughly every
/// [`EXTRACTION_TICK_INTERVAL_MS`] ms.  A typical implementation pulses a GTK
/// `ProgressBar` to keep the UI visually responsive.
///
/// Pass `&|| {}` (a no-op) when no progress feedback is needed.
pub fn install_mod_from_archive_with_nexus_ticking(
    archive_path: &Path,
    game: &Game,
    mod_name: &str,
    strategy: &InstallStrategy,
    nexus_id: Option<u32>,
    tick: &dyn Fn(),
) -> Result<Mod, String> {
    let mod_dir = ModManager::create_mod_directory(game)?;

    match strategy {
        InstallStrategy::Data => {
            if !has_zip_extension(archive_path) {
                install_data_archive_non_zip(archive_path, &mod_dir, tick)?;
            } else {
                install_zip_data_mod(archive_path, &mod_dir, tick)?;
            }
        }
        InstallStrategy::Fomod(files) => {
            // FOMOD destinations are relative to the game's Data folder, so
            // extract directly into `mod_dir/Data/`.
            let data_dir = mod_dir.join("Data");
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("Failed to create Data directory: {e}"))?;
            install_fomod_files(archive_path, &data_dir, files)?;
            // Normalise directory/file names to lowercase so that FOMOD mods
            // are stored consistently alongside non-FOMOD mods on
            // case-sensitive (Linux) filesystems.
            normalize_paths_to_lowercase(&data_dir);
        }
    }

    let mut mod_entry = Mod::new(mod_name, mod_dir);
    mod_entry.installed_from_nexus = nexus_id.is_some();
    mod_entry.nexus_id = nexus_id;

    // Register in the mod database
    let mut db = ModDatabase::load(game);
    // Avoid duplicates
    db.mods.retain(|m| m.name != mod_name);
    db.mods.push(mod_entry.clone());
    db.save(game);

    Ok(mod_entry)
}

/// Extract all files from a zip archive into `dest_dir`, stripping
/// `strip_prefix` from every entry name before writing.
///
/// `strip_prefix` should be the prefix (including trailing `/`) determined by
/// [`find_data_root_in_paths`] or an equivalent detection call.  Pass `""` to
/// extract entries without any prefix stripping.
///
/// `tick` is called approximately every [`EXTRACTION_TICK_INTERVAL_MS`] while
/// iterating over entries so that callers can pulse a progress bar or process
/// UI events to keep the application responsive during extraction of large
/// archives.  Pass `&|| {}` when no progress feedback is needed.
fn extract_zip_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
    tick: &dyn Fn(),
) -> Result<(), String> {
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;

    let mut last_tick = std::time::Instant::now();
    for i in 0..zip.len() {
        // Call tick periodically so the caller can keep the UI responsive.
        let now = std::time::Instant::now();
        if now.duration_since(last_tick).as_millis() as u64 >= EXTRACTION_TICK_INTERVAL_MS {
            tick();
            last_tick = now;
        }

        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Cannot read zip entry: {e}"))?;

        let raw_name = entry.name().to_string();
        // Strip the caller-supplied prefix.
        let rel_name = if !strip_prefix.is_empty() {
            match raw_name.strip_prefix(strip_prefix) {
                Some(r) => r.to_string(),
                None => {
                    // Try a case-insensitive strip for archives where casing
                    // of the prefix may differ from the detected prefix.
                    let raw_lower = raw_name.to_lowercase();
                    let prefix_lower = strip_prefix.to_lowercase();
                    if let Some(r) = raw_lower.strip_prefix(&prefix_lower) {
                        raw_name[raw_name.len() - r.len()..].to_string()
                    } else {
                        // Entry is outside the selected data root – skip it.
                        continue;
                    }
                }
            }
        } else {
            raw_name
        };

        if rel_name.is_empty() || rel_name == "/" {
            continue;
        }

        // Zip-slip protection: reject entries with path traversal components
        if !is_safe_relative_path(&rel_name) {
            log::warn!("Skipping zip entry with unsafe path: {rel_name}");
            continue;
        }

        let out_path = dest_dir.join(&rel_name);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create directory {}: {e}", out_path.display()))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }
            let out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("Failed to create file {}: {e}", out_path.display()))?;
            let mut buffered = BufWriter::with_capacity(EXTRACT_BUFFER_SIZE, out_file);
            std::io::copy(&mut entry, &mut buffered)
                .map_err(|e| format!("Failed to extract {}: {e}", rel_name))?;
        }
    }

    Ok(())
}

fn has_zip_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

fn has_rar_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("rar"))
        .unwrap_or(false)
}

/// Full-archive extraction for non-zip archives (7z, rar, \u2026) using native
/// Rust crates.
///
/// - `.7z` archives are handled by [`sevenz_rust2`].
/// - `.rar` archives are handled by [`unrar`].
///
/// This function may block while decompressing; callers that need to keep the
/// GTK/UI event loop running should call it on a background thread.
fn extract_archive_with_7z(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    if has_rar_extension(archive_path) {
        extract_rar_archive(archive_path, dest_dir)
    } else {
        extract_7z_archive(archive_path, dest_dir)
    }
}

/// Extract a `.7z` archive to `dest_dir` using `sevenz_rust2`.
fn extract_7z_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    sevenz_rust2::decompress_file(archive_path, dest_dir)
        .map_err(|e| format!("Failed to extract 7z archive {}: {e}", archive_path.display()))
}

/// Stream-extract a `.7z` archive directly to `dest_dir`, stripping
/// `strip_prefix` from every stored path before writing.
///
/// Unlike [`extract_7z_archive`] this function uses
/// [`sevenz_rust2::decompress_file_with_extract_fn`] to write each file
/// directly to `dest_dir` without an intermediate temp directory.
/// `strip_prefix` is matched case-insensitively; entries outside the prefix
/// are silently skipped.
///
/// Stored paths that use backslash separators (Windows-created archives) are
/// normalised to forward slashes before any prefix matching or output-path
/// construction, so they are handled correctly on Linux.
///
/// `tick` is called approximately every [`EXTRACTION_TICK_INTERVAL_MS`] for
/// progress feedback.
fn extract_7z_archive_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
    tick: &dyn Fn(),
) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    let prefix_lower = strip_prefix.to_lowercase().replace('\\', "/");
    let dest_dir_buf = dest_dir.to_path_buf();
    let mut last_tick = std::time::Instant::now();

    sevenz_rust2::decompress_file_with_extract_fn(
        archive_path,
        dest_dir,
        |entry, reader, _default_dest| {
            // Periodic tick for progress feedback.
            let now = std::time::Instant::now();
            if now.duration_since(last_tick).as_millis() as u64 >= EXTRACTION_TICK_INTERVAL_MS {
                tick();
                last_tick = now;
            }

            // Normalise the stored name: convert backslash separators from
            // Windows-created archives and strip leading/trailing slashes.
            let raw_name = normalise_path(entry.name());

            // Determine the relative path after stripping the prefix (case-insensitive).
            let rel_name = if prefix_lower.is_empty() {
                raw_name.clone()
            } else {
                let raw_lower = raw_name.to_lowercase().replace('\\', "/");
                match raw_lower.strip_prefix(&prefix_lower) {
                    Some(r) => raw_name[raw_name.len() - r.len()..].to_string(),
                    // Entry is outside the selected prefix – skip it.
                    None => return Ok(true),
                }
            };

            let rel_name = rel_name.trim_start_matches('/').to_string();
            if rel_name.is_empty() {
                // The entry is the prefix directory itself – nothing to write.
                return Ok(true);
            }

            // Zip-slip protection: reject entries that escape the destination.
            if !is_safe_relative_path(&rel_name) {
                log::warn!("Skipping 7z entry with unsafe path: {rel_name}");
                return Ok(true);
            }

            let out_path = dest_dir_buf.join(&rel_name);

            if entry.is_directory() {
                std::fs::create_dir_all(&out_path)?;
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let out_file = std::fs::File::create(&out_path)?;
                let mut buffered = BufWriter::with_capacity(EXTRACT_BUFFER_SIZE, out_file);
                std::io::copy(reader, &mut buffered)?;
            }

            Ok(true)
        },
    )
    .map_err(|e| format!("Failed to extract 7z archive {}: {e}", archive_path.display()))
}

/// Extract a `.rar` archive to `dest_dir` with prefix stripping.
///
/// Extracts the full archive into a temporary directory created inside
/// `dest_dir` (keeping everything on the same filesystem), then moves the
/// appropriate subtree into `dest_dir` using O(1) `rename` syscalls via
/// [`move_dir_contents`].
fn extract_rar_archive_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    // Extract into a temp dir *inside* dest_dir so the two paths share the
    // same filesystem, enabling cheap rename-based moves.
    let tmp = create_temp_extract_dir_in(dest_dir)?;

    extract_rar_archive(archive_path, &tmp)?;

    let result = (|| {
        if strip_prefix.is_empty() {
            return move_dir_contents(&tmp, dest_dir);
        }
        let prefix_trimmed = strip_prefix.trim_end_matches('/');
        let mut src = tmp.clone();
        for component in prefix_trimmed.split('/') {
            if !component.is_empty() {
                src = src.join(component);
            }
        }
        if src.is_dir() {
            move_dir_contents(&src, dest_dir)
        } else {
            // Prefix path not found – fall back to moving the whole extraction.
            move_dir_contents(&tmp, dest_dir)
        }
    })();

    if let Err(e) = std::fs::remove_dir_all(&tmp) {
        log::warn!(
            "Failed to remove temporary extraction directory {}: {e}",
            tmp.display()
        );
    }

    result
}

/// Dispatch prefix-stripped extraction for a non-zip archive (7z or RAR).
///
/// * For `.7z` archives: calls [`extract_7z_archive_to`] which streams files
///   directly to `dest_dir` without any intermediate temp directory.
/// * For `.rar` archives: calls [`extract_rar_archive_to`] which extracts to
///   a temp directory inside `dest_dir` and then renames in place.
fn extract_non_zip_to(
    archive_path: &Path,
    dest_dir: &Path,
    strip_prefix: &str,
    tick: &dyn Fn(),
) -> Result<(), String> {
    if has_rar_extension(archive_path) {
        extract_rar_archive_to(archive_path, dest_dir, strip_prefix)
    } else {
        extract_7z_archive_to(archive_path, dest_dir, strip_prefix, tick)
    }
}

/// Extract a `.rar` archive to `dest_dir` using `unrar`.
fn extract_rar_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(|e| format!("Failed to open RAR archive {}: {e}", archive_path.display()))?;
    loop {
        match archive.read_header() {
            Err(e) => return Err(format!("Failed to read RAR header: {e}")),
            Ok(None) => break,
            Ok(Some(header)) => {
                archive = header.extract_with_base(dest_dir).map_err(|e| {
                    format!("Failed to extract RAR entry: {e}")
                })?;
            }
        }
    }
    Ok(())
}

/// List all file/directory paths stored inside a non-zip archive using native
/// Rust crates (no subprocess).
///
/// - `.7z` archives: parsed with [`sevenz_rust2::ArchiveReader`].
/// - `.rar` archives: iterated with [`unrar::Archive`].
///
/// The returned paths are exactly as stored in the archive (case and
/// separator preserved).
fn list_archive_entries_with_7z(archive_path: &Path) -> Result<Vec<String>, String> {
    if has_rar_extension(archive_path) {
        list_rar_entries(archive_path)
    } else {
        list_7z_entries(archive_path)
    }
}

/// List entries in a `.7z` archive.
fn list_7z_entries(archive_path: &Path) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(archive_path)
        .map_err(|e| format!("Cannot open archive {}: {e}", archive_path.display()))?;
    let reader = sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty())
        .map_err(|e| format!("Failed to read 7z archive {}: {e}", archive_path.display()))?;
    let paths = reader
        .archive()
        .files
        .iter()
        .map(|f| f.name().to_string())
        .collect();
    Ok(paths)
}

/// List entries in a `.rar` archive.
fn list_rar_entries(archive_path: &Path) -> Result<Vec<String>, String> {
    let archive = unrar::Archive::new(archive_path)
        .open_for_listing()
        .map_err(|e| format!("Failed to open RAR archive {}: {e}", archive_path.display()))?;
    let mut paths = Vec::new();
    for entry in archive.flatten() {
        paths.push(entry.filename.to_string_lossy().to_string());
    }
    Ok(paths)
}

/// Extract a single file from a non-zip archive to `dest_dir`, preserving
/// the directory structure from the archive root.
///
/// `file_path_in_archive` must be the exact path as stored in the archive
/// (use [`list_archive_entries_with_7z`] to discover it).  The file will be
/// placed at `dest_dir / file_path_in_archive`.
fn extract_single_file_with_7z(
    archive_path: &Path,
    file_path_in_archive: &str,
    dest_dir: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        format!(
            "Failed to create extraction directory {}: {e}",
            dest_dir.display()
        )
    })?;

    if has_rar_extension(archive_path) {
        extract_single_rar_file(archive_path, file_path_in_archive, dest_dir)
    } else {
        extract_single_7z_file(archive_path, file_path_in_archive, dest_dir)
    }
}

/// Extract a single file from a `.7z` archive using `sevenz_rust2`.
fn extract_single_7z_file(
    archive_path: &Path,
    file_path_in_archive: &str,
    dest_dir: &Path,
) -> Result<(), String> {
    let file = std::fs::File::open(archive_path)
        .map_err(|e| format!("Cannot open archive {}: {e}", archive_path.display()))?;
    let mut reader = sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty())
        .map_err(|e| format!("Failed to read 7z archive {}: {e}", archive_path.display()))?;
    let data = reader.read_file(file_path_in_archive).map_err(|e| {
        format!(
            "Failed to read '{file_path_in_archive}' from {}: {e}",
            archive_path.display()
        )
    })?;
    let out_path = dest_dir.join(Path::new(&normalise_path(file_path_in_archive)));
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directory: {e}"))?;
    }
    std::fs::write(&out_path, &data)
        .map_err(|e| format!("Failed to write extracted file {}: {e}", out_path.display()))
}

/// Extract a single file from a `.rar` archive using `unrar`.
fn extract_single_rar_file(
    archive_path: &Path,
    file_path_in_archive: &str,
    dest_dir: &Path,
) -> Result<(), String> {
    let target_lower = file_path_in_archive.to_lowercase().replace('\\', "/");
    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(|e| format!("Failed to open RAR archive {}: {e}", archive_path.display()))?;
    loop {
        match archive.read_header() {
            Err(e) => return Err(format!("Failed to read RAR header: {e}")),
            Ok(None) => break,
            Ok(Some(header)) => {
                let entry_name = header.entry().filename.to_string_lossy().to_string();
                let entry_lower = entry_name.to_lowercase().replace('\\', "/");
                if entry_lower == target_lower {
                    archive = header.extract_with_base(dest_dir).map_err(|e| {
                        format!("Failed to extract RAR entry '{entry_name}': {e}")
                    })?;
                    return Ok(());
                }
                archive = header.skip().map_err(|e| {
                    format!("Failed to skip RAR entry '{entry_name}': {e}")
                })?;
            }
        }
    }
    Err(format!(
        "File '{file_path_in_archive}' not found in RAR archive {}",
        archive_path.display()
    ))
}

/// Install a zip-format Data mod into `mod_dir`.
///
/// Uses the scoring heuristic ([`find_data_root_in_paths`]) to detect the
/// archive's data root, then handles three cases:
///
/// 1. **BAIN packages** – all numbered top-level directories are extracted and
///    merged into `mod_dir/Data/` in ascending order.
/// 2. **Root-level mod** (e.g. ENB, ReShade) – content is extracted into
///    `mod_dir/` alongside an empty `mod_dir/Data/`; the deployment layer
///    ([`link_items_alongside_data`]) will link these files to the game root.
/// 3. **Normal data mod** – content is extracted with the detected prefix
///    stripped directly into `mod_dir/Data/`.
fn install_zip_data_mod(archive_path: &Path, mod_dir: &Path, tick: &dyn Fn()) -> Result<(), String> {
    // Collect all file names from the central directory (no decompression).
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;
    let all_paths: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    drop(zip); // release the file handle before extraction
    let path_refs: Vec<&str> = all_paths.iter().map(|s| s.as_str()).collect();

    let data_dir = mod_dir.join("Data");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("Failed to create Data directory: {e}"))?;

    if is_bain_archive(&path_refs) {
        // BAIN: install all numbered packages, merged in ascending order so
        // that higher-numbered packages override lower-numbered ones.
        for bain_dir in collect_bain_top_dirs(&path_refs) {
            let bain_prefix = format!("{bain_dir}/");
            extract_zip_to(archive_path, &data_dir, &bain_prefix, tick)?;
        }
        normalize_paths_to_lowercase(&data_dir);
        return Ok(());
    }

    let data_root = find_data_root_in_paths(&path_refs);
    let root_trimmed = data_root.trim_end_matches('/');
    let best_score = score_as_data_root(root_trimmed, &path_refs);

    if best_score > 0 {
        // Scoring found a clear data root: extract into mod_dir/Data/ with
        // the detected prefix stripped.
        extract_zip_to(archive_path, &data_dir, &data_root, tick)?;
        normalize_paths_to_lowercase(&data_dir);
        return Ok(());
    }

    // No data-like content found via scoring.  Fall back to the simple
    // common-prefix approach and check for root-level markers.
    let simple_prefix = find_common_prefix_from_paths(&path_refs);
    let stripped: Vec<String> = path_refs
        .iter()
        .filter_map(|p| {
            let p_lower = p.to_lowercase().replace('\\', "/");
            let p_lower = p_lower.trim_start_matches('/').to_string();
            if simple_prefix.is_empty() {
                Some(p_lower)
            } else {
                p_lower
                    .strip_prefix(&simple_prefix.to_lowercase())
                    .map(|s| s.to_string())
            }
        })
        .collect();
    let stripped_refs: Vec<&str> = stripped.iter().map(|s| s.as_str()).collect();

    if looks_like_root_level_mod(&stripped_refs) {
        // Root-level mod (ENB, ReShade, …): extract to mod_dir/ so that the
        // deploy layer can link these files to the game root directory.
        // mod_dir/Data/ is already created above so enable_mod will take the
        // "has Data dir" branch and call link_items_alongside_data.
        extract_zip_to(archive_path, mod_dir, &simple_prefix, tick)?;
    } else {
        // Ambiguous – just extract everything to mod_dir/Data/.
        extract_zip_to(archive_path, &data_dir, &simple_prefix, tick)?;
        normalize_paths_to_lowercase(&data_dir);
    }

    Ok(())
}

fn install_data_archive_non_zip(
    archive_path: &Path,
    mod_dir: &Path,
    tick: &dyn Fn(),
) -> Result<(), String> {
    // Pre-list archive entries to determine the install strategy before
    // performing the (potentially large) extraction.
    let entries = list_archive_entries_with_7z(archive_path).unwrap_or_default();
    let path_refs: Vec<&str> = entries.iter().map(|s| s.as_str()).collect();

    let data_dir = mod_dir.join("Data");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("Failed to create Data directory: {e}"))?;

    let install_result = (|| -> Result<(), String> {
        if is_bain_archive(&path_refs) {
            // BAIN: merge all numbered packages in ascending order.
            for bain_dir in collect_bain_top_dirs(&path_refs) {
                let bain_prefix = format!("{bain_dir}/");
                extract_non_zip_to(archive_path, &data_dir, &bain_prefix, tick)?;
            }
            return Ok(());
        }

        let data_root = find_data_root_in_paths(&path_refs);
        let root_trimmed = data_root.trim_end_matches('/');
        let best_score = score_as_data_root(root_trimmed, &path_refs);

        if best_score > 0 {
            // Scoring found the data root: stream directly into mod_dir/Data/.
            extract_non_zip_to(archive_path, &data_dir, &data_root, tick)?;
            return Ok(());
        }

        // Fallback: use simple common prefix, check for root-level markers.
        let simple_prefix = find_common_prefix_from_paths(&path_refs);
        let stripped: Vec<String> = path_refs
            .iter()
            .filter_map(|p| {
                let p_lower = p.to_lowercase().replace('\\', "/");
                let p_lower = p_lower.trim_start_matches('/').to_string();
                if simple_prefix.is_empty() {
                    Some(p_lower)
                } else {
                    p_lower
                        .strip_prefix(&simple_prefix.to_lowercase())
                        .map(|s| s.to_string())
                }
            })
            .collect();
        let stripped_refs: Vec<&str> = stripped.iter().map(|s| s.as_str()).collect();

        if looks_like_root_level_mod(&stripped_refs) {
            extract_non_zip_to(archive_path, mod_dir, &simple_prefix, tick)?;
        } else {
            extract_non_zip_to(archive_path, &data_dir, &simple_prefix, tick)?;
        }
        Ok(())
    })();

    // Normalize folder / file names inside Data/ to lowercase so that mods
    // with mixed-case directories (TEXTURES/, Meshes/, …) are stored
    // consistently on case-sensitive (Linux) filesystems.
    if data_dir.is_dir() {
        normalize_paths_to_lowercase(&data_dir);
    }

    install_result
}

fn create_temp_extract_dir() -> Result<std::path::PathBuf, String> {
    let base = std::env::temp_dir();
    for attempt in 0..100u32 {
        let temp_extract_path = base.join(format!(
            "linkmm_extract_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            attempt
        ));
        match std::fs::create_dir(&temp_extract_path) {
            Ok(()) => return Ok(temp_extract_path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to create temporary extraction directory {}: {e}",
                    temp_extract_path.display()
                ));
            }
        }
    }
    Err("Failed to allocate a temporary extraction directory".to_string())
}

/// Like [`create_temp_extract_dir`] but creates the temp directory *inside*
/// `parent`, keeping it on the same filesystem as the final destination.
/// This allows rename-based (O(1)) moves rather than cross-device copies.
fn create_temp_extract_dir_in(parent: &Path) -> Result<std::path::PathBuf, String> {
    for attempt in 0..100u32 {
        let temp_extract_path = parent.join(format!(
            ".linkmm_tmp_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            attempt
        ));
        match std::fs::create_dir(&temp_extract_path) {
            Ok(()) => return Ok(temp_extract_path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to create temporary extraction directory {}: {e}",
                    temp_extract_path.display()
                ));
            }
        }
    }
    Err("Failed to allocate a temporary extraction directory".to_string())
}

/// Move all direct children of `src` into `dst` using `rename`.
///
/// When `src` and `dst` are on the same filesystem, `rename` is a single
/// directory-entry update — orders of magnitude faster than a byte copy.
/// Falls back to a copy if the rename fails (e.g. cross-device move).
fn move_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create destination {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| format!("Failed to read {}: {e}", src.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        // Try a fast rename first; fall back to copy+delete on cross-device error.
        if let Err(_) = std::fs::rename(&from, &to) {
            if from.is_dir() {
                copy_dir_contents(&from, &to)?;
                let _ = std::fs::remove_dir_all(&from);
            } else if from.is_file() {
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent dir: {e}"))?;
                }
                std::fs::copy(&from, &to).map_err(|e| {
                    format!("Failed to move {} → {}: {e}", from.display(), to.display())
                })?;
                let _ = std::fs::remove_file(&from);
            }
        }
    }
    Ok(())
}

fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create destination {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_contents(&from, &to)?;
        } else if from.is_file() {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir {}: {e}", parent.display()))?;
            }
            std::fs::copy(&from, &to).map_err(|e| {
                format!(
                    "Failed to copy extracted file {} -> {}: {e}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Recursively rename every file and directory inside `dir` to use lowercase
/// names.  The directory `dir` itself is never renamed.
///
/// When both an uppercase and a lowercase variant of a name already exist in
/// the same directory (e.g. `TEXTURES/` and `textures/`), the two directories
/// are merged: contents of the uppercase one are moved into the lowercase one
/// and the now-empty uppercase directory is removed.  For files the lowercase
/// version takes precedence and the uppercase duplicate is discarded.
///
/// Any rename or merge failure is logged as a warning but does not abort the
/// operation so that as many entries as possible are normalised.
fn normalize_paths_to_lowercase(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            log::warn!("normalize_paths_to_lowercase: cannot read {}: {err}", dir.display());
            return;
        }
    };

    // Collect all entries first to avoid iterator invalidation during renames.
    let items: Vec<_> = entries.flatten().collect();

    for entry in items {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let lower = name_str.to_lowercase();

        if name_str.as_ref() == lower.as_str() {
            // Already lowercase – recurse into directories.
            if path.is_dir() {
                normalize_paths_to_lowercase(&path);
            }
            continue;
        }

        let new_path = dir.join(&lower);

        if new_path.exists() {
            // A lowercase-named item already exists at the destination.
            if path.is_dir() && new_path.is_dir() {
                // Merge: move all children of the uppercase dir into the
                // lowercase dir, then remove the (now empty) uppercase dir.
                if let Ok(children) = std::fs::read_dir(&path) {
                    for child in children.flatten() {
                        let child_dst = new_path.join(child.file_name());
                        if let Err(e) = std::fs::rename(child.path(), &child_dst) {
                            log::warn!(
                                "normalize_paths_to_lowercase: failed to merge {} -> {}: {e}",
                                child.path().display(),
                                child_dst.display()
                            );
                        }
                    }
                }
                let _ = std::fs::remove_dir(&path);
                normalize_paths_to_lowercase(&new_path);
            }
            // For files, the lowercase version already exists – discard the
            // uppercase duplicate.
        } else {
            // Safe to rename: lowercase version does not yet exist.
            if let Err(e) = std::fs::rename(&path, &new_path) {
                log::warn!(
                    "normalize_paths_to_lowercase: failed to rename {} -> {}: {e}",
                    path.display(),
                    new_path.display()
                );
                // Still recurse into the original path if it is a directory.
                if path.is_dir() {
                    normalize_paths_to_lowercase(&path);
                }
                continue;
            }
            if new_path.is_dir() {
                normalize_paths_to_lowercase(&new_path);
            }
        }
    }
}

/// Detect whether all entries in the archive share a common top-level directory
/// prefix.  If so, return it (with trailing `/`).
///
/// Uses `ZipArchive::file_names()` which only reads the central directory
/// metadata (no decompression), so this is fast and does not require a
/// mutable borrow.
fn find_common_prefix(zip: &zip::ZipArchive<std::fs::File>) -> String {
    if zip.is_empty() {
        return String::new();
    }

    let mut first_top: Option<String> = None;
    let mut all_same = true;

    for name in zip.file_names() {
        let top = name.split('/').next().unwrap_or("");
        if top.is_empty() {
            continue;
        }
        match &first_top {
            None => first_top = Some(top.to_string()),
            Some(ft) if ft != top => {
                all_same = false;
                break;
            }
            _ => {}
        }
    }

    if all_same {
        if let Some(ft) = first_top {
            // Only strip if there are entries *inside* the folder (not just the folder itself)
            if zip.len() > 1 {
                return format!("{ft}/");
            }
        }
    }
    String::new()
}

/// Install FOMOD-selected files from an archive.
///
/// Supports both zip archives and non-zip archives (7z, rar, etc.).  For
/// non-zip archives the archive is first fully extracted to a temporary
/// directory; the per-file selection logic is then applied to the extracted
/// tree before the temp directory is removed.
///
/// For each `FomodFile`, extract the `source` path from the archive and place it
/// at `dest_dir / destination`.
///
/// `tick` is called periodically during the zip extraction phase so the
/// caller can pulse a progress indicator.  Pass `&|| {}` when not needed.
/// For non-zip archives (7z, rar, …) this function runs the extraction
/// synchronously; the caller is responsible for offloading to a background
/// thread to keep the UI responsive.
fn install_fomod_files(
    archive_path: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    if !has_zip_extension(archive_path) {
        return install_fomod_files_non_zip(archive_path, dest_dir, files);
    }
    let file =
        std::fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("Cannot read zip archive: {e}"))?;
    let archive_prefix = normalise_path(&find_common_prefix(&zip));
    let archive_prefix_lower = archive_prefix.to_lowercase();

    // Build a map of lowercased entry names → indices for case-insensitive
    // matching.
    let mut entry_map: Vec<(String, String, usize)> = Vec::new(); // (lower, original, index)
    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else {
            continue;
        };
        let name = entry.name().to_string();
        let lower = name.to_lowercase();
        entry_map.push((lower, name, i));
    }

    // Sort files by priority (higher priority wins for same destination)
    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|a, b| a.priority.cmp(&b.priority));

    for fomod_file in &sorted_files {
        let source = normalise_path(&fomod_file.source);
        // Strip a leading "Data/" segment: FOMOD destinations are relative to
        // the game root, but dest_dir is already mod_dir/Data/.  Without this
        // stripping a destination of "Data/textures" would produce the double-
        // nested mod_dir/Data/Data/textures/ layout.
        let destination = strip_data_prefix(&normalise_path(&fomod_file.destination));
        let source_lower = source.to_lowercase();

        // Find matching entry indices. Some FOMODs reference files under
        // "Data/..." while the archive has the same files at root (or vice
        // versa), so try fallback source aliases.
        let mut matched_source = source.clone();
        let mut matching_indices = collect_matching_entries(&entry_map, &source_lower);

        if matching_indices.is_empty() {
            if !archive_prefix_lower.is_empty() {
                let wrapped_source_lower = format!("{archive_prefix_lower}/{source_lower}");
                matching_indices = collect_matching_entries(&entry_map, &wrapped_source_lower);
                if !matching_indices.is_empty() {
                    matched_source = format!("{archive_prefix}/{source}");
                }
            }
        }

        if matching_indices.is_empty() {
            let stripped_source = strip_data_prefix(&source);
            let stripped_lower = stripped_source.to_lowercase();
            if !stripped_source.is_empty() && stripped_lower != source_lower {
                matching_indices = collect_matching_entries(&entry_map, &stripped_lower);
                if !matching_indices.is_empty() {
                    matched_source = stripped_source;
                } else if !archive_prefix_lower.is_empty() {
                    let wrapped_stripped_lower = format!("{archive_prefix_lower}/{stripped_lower}");
                    matching_indices = collect_matching_entries(&entry_map, &wrapped_stripped_lower);
                    if !matching_indices.is_empty() {
                        matched_source = format!("{archive_prefix}/{stripped_source}");
                    }
                }
            } else if source_lower == "data" || source_lower == "data/" {
                matching_indices = entry_map
                    .iter()
                    .filter(|(nl, _, _)| {
                        !(nl == "fomod" || nl.starts_with("fomod/") || nl.contains("/fomod/"))
                    })
                    .map(|(_, orig, idx)| (orig.clone(), *idx))
                    .collect();
                if !matching_indices.is_empty() {
                    matched_source = String::new();
                }
            }
        }

        if matching_indices.is_empty() && !source_lower.is_empty() {
            let prefixed = format!("data/{source_lower}");
            matching_indices = collect_matching_entries(&entry_map, &prefixed);
            if !matching_indices.is_empty() {
                matched_source = prefixed;
            } else if !archive_prefix_lower.is_empty() {
                let wrapped_prefixed = format!("{archive_prefix_lower}/{prefixed}");
                matching_indices = collect_matching_entries(&entry_map, &wrapped_prefixed);
                if !matching_indices.is_empty() {
                    matched_source = format!("{archive_prefix}/data/{source}");
                }
            }
        }

        let matched_source_lower = matched_source.to_lowercase();

        for (entry_name, entry_idx) in matching_indices {
            let entry_lower = entry_name.to_lowercase();
            // Compute the relative portion after the source prefix
            let rel = if entry_lower == matched_source_lower {
                // Single file
                Path::new(&entry_name)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
            } else {
                entry_name[matched_source.len()..]
                    .trim_start_matches('/')
                    .to_string()
            };

            if rel.is_empty() {
                continue;
            }

            // Zip-slip protection on the combined destination + rel path
            let combined = if destination.is_empty() {
                std::borrow::Cow::Borrowed(rel.as_str())
            } else {
                std::borrow::Cow::Owned(format!("{destination}/{rel}"))
            };
            if !is_safe_relative_path(combined.as_ref()) {
                log::warn!("Skipping fomod entry with unsafe path: {combined}");
                continue;
            }

            let out_path = dest_dir.join(&destination).join(&rel);

            // Use by_index to avoid long-lived borrow issues
            let mut entry = zip
                .by_index(entry_idx)
                .map_err(|e| format!("Cannot read entry {entry_name}: {e}"))?;

            if entry.is_dir() {
                continue;
            }

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }

            let out_file =
                std::fs::File::create(&out_path).map_err(|e| format!("Failed to create file: {e}"))?;
            let mut buffered = BufWriter::with_capacity(EXTRACT_BUFFER_SIZE, out_file);
            std::io::copy(&mut entry, &mut buffered).map_err(|e| format!("Failed to extract: {e}"))?;
        }
    }

    Ok(())
}

/// Extract a non-zip archive to a temporary directory, run the FOMOD file
/// selection logic on the extracted tree, then remove the temp directory.
fn install_fomod_files_non_zip(
    archive_path: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    let tmp = create_temp_extract_dir()?;
    extract_archive_with_7z(archive_path, &tmp)?;
    let result = install_fomod_files_from_dir(&tmp, dest_dir, files);
    if let Err(e) = std::fs::remove_dir_all(&tmp) {
        log::warn!(
            "Failed to remove temporary extraction directory {}: {e}",
            tmp.display()
        );
    }
    result
}

/// Install FOMOD-selected files from an already-extracted directory tree.
///
/// Mirrors the matching and extraction logic of [`install_fomod_files`] but
/// operates on real filesystem paths instead of a zip archive.
fn install_fomod_files_from_dir(
    extracted_dir: &Path,
    dest_dir: &Path,
    files: &[FomodFile],
) -> Result<(), String> {
    // Build a map of (lowercase_rel, original_rel) for all entries in the tree.
    let mut entry_map: Vec<(String, String)> = Vec::new();
    collect_fs_entries(extracted_dir, extracted_dir, &mut entry_map);

    let (archive_prefix, archive_prefix_lower) = find_fs_common_prefix(&entry_map);

    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|a, b| a.priority.cmp(&b.priority));

    for fomod_file in &sorted_files {
        let source = normalise_path(&fomod_file.source);
        let destination = strip_data_prefix(&normalise_path(&fomod_file.destination));
        let source_lower = source.to_lowercase();

        let mut matched_source = source.clone();
        let mut matching_rels = collect_matching_fs_entries(&entry_map, &source_lower);

        if matching_rels.is_empty() && !archive_prefix_lower.is_empty() {
            let wrapped = format!("{archive_prefix_lower}{source_lower}");
            matching_rels = collect_matching_fs_entries(&entry_map, &wrapped);
            if !matching_rels.is_empty() {
                matched_source = format!("{archive_prefix}{source}");
            }
        }

        if matching_rels.is_empty() {
            let stripped = strip_data_prefix(&source);
            let stripped_lower = stripped.to_lowercase();
            if !stripped.is_empty() && stripped_lower != source_lower {
                matching_rels = collect_matching_fs_entries(&entry_map, &stripped_lower);
                if !matching_rels.is_empty() {
                    matched_source = stripped;
                } else if !archive_prefix_lower.is_empty() {
                    let wrapped_stripped = format!("{archive_prefix_lower}{stripped_lower}");
                    matching_rels = collect_matching_fs_entries(&entry_map, &wrapped_stripped);
                    if !matching_rels.is_empty() {
                        matched_source = format!("{archive_prefix}{stripped}");
                    }
                }
            } else if source_lower == "data" || source_lower == "data/" {
                matching_rels = entry_map
                    .iter()
                    .filter(|(nl, _)| {
                        !(nl == "fomod"
                            || nl.starts_with("fomod/")
                            || nl.contains("/fomod/"))
                    })
                    .map(|(_, orig)| orig.clone())
                    .collect();
                if !matching_rels.is_empty() {
                    matched_source = String::new();
                }
            }
        }

        if matching_rels.is_empty() && !source_lower.is_empty() {
            let prefixed = format!("data/{source_lower}");
            matching_rels = collect_matching_fs_entries(&entry_map, &prefixed);
            if !matching_rels.is_empty() {
                matched_source = prefixed;
            } else if !archive_prefix_lower.is_empty() {
                let wrapped_prefixed = format!("{archive_prefix_lower}data/{source_lower}");
                matching_rels = collect_matching_fs_entries(&entry_map, &wrapped_prefixed);
                if !matching_rels.is_empty() {
                    matched_source = format!("{archive_prefix}data/{source}");
                }
            }
        }

        let matched_source_lower = matched_source.to_lowercase();

        for orig_rel in matching_rels {
            let full_path = extracted_dir.join(&orig_rel);

            // Skip directory entries – only copy actual files.
            if full_path.is_dir() {
                continue;
            }

            let entry_lower = orig_rel.to_lowercase();
            let rel = if entry_lower == matched_source_lower {
                // Single-file match: use the file name only.
                Path::new(&orig_rel)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
            } else {
                orig_rel[matched_source.len()..]
                    .trim_start_matches('/')
                    .to_string()
            };

            if rel.is_empty() {
                continue;
            }

            // Zip-slip protection on the combined destination + rel path.
            let combined = if destination.is_empty() {
                std::borrow::Cow::Borrowed(rel.as_str())
            } else {
                std::borrow::Cow::Owned(format!("{destination}/{rel}"))
            };
            if !is_safe_relative_path(combined.as_ref()) {
                log::warn!("Skipping fomod entry with unsafe path: {combined}");
                continue;
            }

            let out_path = dest_dir.join(&destination).join(&rel);

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }

            std::fs::copy(&full_path, &out_path).map_err(|e| {
                format!(
                    "Failed to copy {} → {}: {e}",
                    full_path.display(),
                    out_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn normalise_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.strip_prefix('/').unwrap_or(&s);
    s.trim_end_matches('/').to_string()
}

fn collect_matching_entries(
    entry_map: &[(String, String, usize)],
    source_lower: &str,
) -> Vec<(String, usize)> {
    entry_map
        .iter()
        .filter(|(nl, _, _)| {
            *nl == source_lower
                || nl.starts_with(&format!("{source_lower}/"))
                || nl.starts_with(&format!("{source_lower}\\"))
        })
        .map(|(_, orig, idx)| (orig.clone(), *idx))
        .collect()
}

/// Strip a leading `Data/` segment (case-insensitive) from a FOMOD destination
/// path.
///
/// FOMOD `destination` attributes are relative to the **game root**, so a value
/// of `"Data/textures"` means the game's `Data/textures` folder.  Because
/// [`install_fomod_files`] already extracts into `mod_dir/Data/`, including the
/// `Data/` segment verbatim would produce `mod_dir/Data/Data/textures/` — the
/// classic double-nesting bug.  Stripping the leading `Data/` (or bare `Data`)
/// avoids this.
fn strip_data_prefix(s: &str) -> String {
    let lower = s.to_lowercase();
    if lower == "data" || lower == "data/" {
        String::new()
    } else if lower.starts_with("data/") {
        s["data/".len()..].to_string()
    } else {
        s.to_string()
    }
}

/// Check that a relative path is safe (no traversal above the root).
///
/// Rejects paths containing `..` components that would escape the destination.
fn is_safe_relative_path(path: &str) -> bool {
    use std::path::Component;
    let normalised = path.replace('\\', "/");
    let p = Path::new(&normalised);
    let mut depth: i32 = 0;
    for component in p.components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::Normal(_) => {
                depth += 1;
            }
            Component::RootDir | Component::Prefix(_) => {
                // Absolute paths are not safe relative paths
                return false;
            }
            Component::CurDir => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    #[test]
    fn list_archive_entries_with_7z_returns_paths() {
        let tmp = tempdir();
        let Some(archive) = create_test_7z(&tmp, "list_test.7z") else {
            return;
        };
        let entries = list_archive_entries_with_7z(&archive).unwrap();
        let lower: Vec<String> = entries.iter().map(|p| p.to_lowercase().replace('\\', "/")).collect();
        assert!(lower.iter().any(|p| p.ends_with("fomod/moduleconfig.xml")),
            "expected fomod/moduleconfig.xml in listing, got: {lower:?}");
        assert!(lower.iter().any(|p| p.ends_with("data/textures/sky.dds")),
            "expected Data/textures/sky.dds in listing, got: {lower:?}");
    }

        /// Create a simple zip archive in `dir` containing the given entries.
    /// Each entry is (name, content).  Names ending with `/` are directories.
    fn create_test_zip(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let archive_path = dir.join("test_mod.zip");
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
        // finish() returns the inner File – drop it to flush
        let inner = zip_writer.finish().unwrap();
        drop(inner);
        archive_path
    }

    fn create_test_7z(root: &Path, archive_name: &str) -> Option<PathBuf> {
        let archive_path = root.join(archive_name);
        let staging = root.join("staging");
        std::fs::create_dir_all(staging.join("fomod")).ok()?;
        std::fs::create_dir_all(staging.join("Data/textures")).ok()?;
        std::fs::write(
            staging.join("fomod/ModuleConfig.xml"),
            r#"<config>
  <requiredInstallFiles>
    <file source="Data/textures/sky.dds" destination="Data/textures/sky.dds" />
  </requiredInstallFiles>
</config>"#,
        )
        .ok()?;
        std::fs::write(staging.join("Data/textures/sky.dds"), b"dds").ok()?;
        let out_file = std::fs::File::create(&archive_path).ok()?;
        sevenz_rust2::compress(staging.as_path(), out_file).ok()?;
        Some(archive_path)
    }

    /// Create a 7z archive containing a FOMOD image for testing.
    ///
    /// Layout:
    /// ```
    /// fomod/images/preview.png  (content: "pngdata")
    /// Data/textures/sky.dds
    /// ```
    fn create_test_7z_with_image(root: &Path, archive_name: &str) -> Option<PathBuf> {
        let archive_path = root.join(archive_name);
        let staging = root.join("staging_img");
        std::fs::create_dir_all(staging.join("fomod/images")).ok()?;
        std::fs::create_dir_all(staging.join("Data/textures")).ok()?;
        std::fs::write(staging.join("fomod/images/preview.png"), b"pngdata").ok()?;
        std::fs::write(staging.join("Data/textures/sky.dds"), b"dds").ok()?;
        let out_file = std::fs::File::create(&archive_path).ok()?;
        sevenz_rust2::compress(staging.as_path(), out_file).ok()?;
        Some(archive_path)
    }

    #[test]
    fn detect_strategy_data_for_loose_files() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[("textures/sky.dds", b"dds"), ("meshes/rock.nif", b"nif")],
        );
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Data));
    }

    #[test]
    fn detect_strategy_data_for_archive_with_data_folder() {
        // Archives that already contain a Data/ subfolder now also return Data,
        // not Root – the distinction is handled during extraction.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("Data/", b""),
                ("Data/textures/sky.dds", b"dds"),
                ("Data/meshes/rock.nif", b"nif"),
            ],
        );
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Data));
    }

    #[test]
    fn detect_strategy_non_zip_defaults_to_data() {
        let tmp = tempdir();
        let archive = tmp.join("mod.7z");
        std::fs::write(&archive, b"fake").unwrap();
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Data));
    }

    #[test]
    fn detect_strategy_non_zip_with_fomod_uses_fomod_strategy() {
        let tmp = tempdir();
        let Some(archive) = create_test_7z(&tmp, "mod.7z") else {
            return;
        };
        let strategy = detect_strategy(&archive).unwrap();
        assert!(matches!(strategy, InstallStrategy::Fomod(_)));
    }

    #[test]
    fn parse_fomod_from_archive_reads_non_zip_module_config() {
        let tmp = tempdir();
        let Some(archive) = create_test_7z(&tmp, "mod.7z") else {
            return;
        };
        let cfg = parse_fomod_from_archive(&archive).unwrap();
        assert!(!cfg.required_files.is_empty());
    }

    #[test]
    fn archive_has_data_folder_flat_content() {
        // Archive without Data/ subfolder – helper returns false.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[("textures/sky.dds", b"dds"), ("plugin.esp", b"esp")],
        );
        assert!(!archive_has_data_folder(&archive));
    }

    #[test]
    fn archive_has_data_folder_direct_data_prefix() {
        // Archive where Data/ is the *only* top-level folder: find_common_prefix
        // strips it, so after stripping the remaining entries no longer start
        // with Data/ → helper returns false → content is extracted into Data/.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("Data/", b""),
                ("Data/textures/sky.dds", b"dds"),
                ("Data/meshes/rock.nif", b"nif"),
            ],
        );
        assert!(!archive_has_data_folder(&archive));
    }

    #[test]
    fn archive_has_data_folder_wrapped_data() {
        // Archive with a wrapper folder containing Data/ – after stripping the
        // wrapper the remaining path starts with Data/ → helper returns true.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("MyMod/", b""),
                ("MyMod/Data/", b""),
                ("MyMod/Data/textures/sky.dds", b"dds"),
            ],
        );
        assert!(archive_has_data_folder(&archive));
    }

    #[test]
    fn install_flat_archive_places_files_under_data_subdir() {
        // Flat archive (textures/sky.dds) should land in dest/Data/textures/sky.dds
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("textures/sky.dds", b"dds_data"),
                ("plugin.esp", b"esp_data"),
            ],
        );
        let dest = tmp.join("mod_dir");
        std::fs::create_dir_all(&dest).unwrap();
        let data_dest = dest.join("Data");
        std::fs::create_dir_all(&data_dest).unwrap();
        extract_zip_to(&archive, &data_dest, "", &|| {}).unwrap();

        assert!(dest.join("Data").join("textures").join("sky.dds").exists());
        assert!(dest.join("Data").join("plugin.esp").exists());
        assert_eq!(
            std::fs::read_to_string(dest.join("Data").join("plugin.esp")).unwrap(),
            "esp_data"
        );
    }

    #[test]
    fn install_data_folder_archive_preserves_data_subdir() {
        // Archive Data/textures/sky.dds: find_common_prefix strips Data/ so
        // extraction to mod_dir/Data/ gives mod_dir/Data/textures/sky.dds.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[("Data/", b""), ("Data/textures/sky.dds", b"dds_data")],
        );
        let dest = tmp.join("mod_dir");
        std::fs::create_dir_all(&dest).unwrap();
        // archive_has_data_folder returns false for this archive (Data/ stripped)
        // so installer extracts to dest/Data/.
        assert!(!archive_has_data_folder(&archive));
        let data_dest = dest.join("Data");
        std::fs::create_dir_all(&data_dest).unwrap();
        extract_zip_to(&archive, &data_dest, "Data/", &|| {}).unwrap();

        assert!(dest.join("Data").join("textures").join("sky.dds").exists());
    }

    #[test]
    fn install_wrapped_data_archive_preserves_data_subdir() {
        // Archive MyMod/Data/textures/sky.dds: prefix MyMod/ stripped, remaining
        // starts with Data/ → extract to mod_dir/ → mod_dir/Data/textures/sky.dds.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("MyMod/", b""),
                ("MyMod/Data/", b""),
                ("MyMod/Data/textures/sky.dds", b"dds_data"),
            ],
        );
        let dest = tmp.join("mod_dir");
        std::fs::create_dir_all(&dest).unwrap();
        assert!(archive_has_data_folder(&archive));
        extract_zip_to(&archive, &dest, "MyMod/", &|| {}).unwrap();

        assert!(dest.join("Data").join("textures").join("sky.dds").exists());
    }

    #[test]
    fn extract_zip_strips_common_prefix() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("MyMod/", b""),
                ("MyMod/textures/sky.dds", b"dds_data"),
                ("MyMod/plugin.esp", b"esp_data"),
            ],
        );
        let dest = tmp.join("extracted");
        std::fs::create_dir_all(&dest).unwrap();
        extract_zip_to(&archive, &dest, "MyMod/", &|| {}).unwrap();
        assert!(dest.join("textures").join("sky.dds").exists());
        assert!(dest.join("plugin.esp").exists());
    }

    #[test]
    fn is_safe_relative_path_rejects_traversal() {
        assert!(!is_safe_relative_path("../etc/passwd"));
        assert!(!is_safe_relative_path("foo/../../bar"));
        assert!(!is_safe_relative_path("/absolute/path"));
        assert!(is_safe_relative_path("foo/bar/baz"));
        assert!(is_safe_relative_path("textures/sky.dds"));
        assert!(is_safe_relative_path("a/../a/b")); // depth never goes negative
    }

    #[test]
    fn strip_data_prefix_removes_leading_data_segment() {
        // Bare "Data" with various casings / trailing slashes.
        assert_eq!(strip_data_prefix("Data"), "");
        assert_eq!(strip_data_prefix("data"), "");
        assert_eq!(strip_data_prefix("DATA"), "");
        assert_eq!(strip_data_prefix("Data/"), "");
        // Leading "Data/" prefix is stripped; rest is preserved.
        assert_eq!(strip_data_prefix("Data/textures"), "textures");
        assert_eq!(strip_data_prefix("data/Textures/sky"), "Textures/sky");
        assert_eq!(strip_data_prefix("DATA/meshes/rock.nif"), "meshes/rock.nif");
        // Non-Data paths are returned unchanged.
        assert_eq!(strip_data_prefix("textures/sky.dds"), "textures/sky.dds");
        assert_eq!(strip_data_prefix(""), "");
        assert_eq!(strip_data_prefix("SomeOtherFolder"), "SomeOtherFolder");
    }

    #[test]
    fn install_fomod_files_falls_back_when_source_uses_data_prefix() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
                ("textures/sky.dds", b"dds_data"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "Data/textures".to_string(),
                destination: "Data/textures".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("textures").join("sky.dds").exists());
    }

    #[test]
    fn install_fomod_files_handles_source_with_trailing_slash() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("meshes/", b""),
                ("meshes/armor.nif", b"nif_data"),
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "meshes/".to_string(),
                destination: "Data/meshes".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("meshes").join("armor.nif").exists());
    }

    #[test]
    fn install_fomod_files_data_root_fallback_skips_fomod_config_dir() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
                ("plugin.esp", b"esp_data"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "Data".to_string(),
                destination: "Data".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("plugin.esp").exists());
        assert!(!dest.join("fomod").exists());
    }

    #[test]
    fn install_fomod_files_skips_empty_directory_entries() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("textures/", b""),
                ("textures/armor.dds", b"dds_data"),
                ("textures/empty/", b""),
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "textures".to_string(),
                destination: "Data/textures".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("textures").join("armor.dds").exists());
        assert!(!dest.join("textures").join("empty").exists());
    }

    #[test]
    fn install_fomod_files_matches_sources_in_wrapped_archives() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("Aela Replacer/", b""),
                ("Aela Replacer/00 main/", b""),
                ("Aela Replacer/00 main/AelaStandalone.esp", b"esp_data"),
                (
                    "Aela Replacer/00 main/textures/Actors/Character/Aela/Head/femalehead.dds",
                    b"dds_data",
                ),
                ("Aela Replacer/fomod/", b""),
                ("Aela Replacer/fomod/ModuleConfig.xml", b"<config/>"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "00 main".to_string(),
                destination: "Data".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("AelaStandalone.esp").exists());
        assert!(
            dest.join("textures")
                .join("Actors")
                .join("Character")
                .join("Aela")
                .join("Head")
                .join("femalehead.dds")
                .exists()
        );
    }

    #[test]
    fn parse_fomod_xml_parses_plugin_dependencies_flags_and_image() {
        let xml = br#"
            <config>
              <moduleName>Example Mod</moduleName>
              <installSteps>
                <installStep name="Variants">
                  <optionalFileGroups>
                    <group name="Main" type="SelectAny">
                      <plugins>
                        <plugin name="Plus Variant">
                          <description>Use plus variant</description>
                          <image path="images/plus.png"/>
                          <conditionFlags>
                            <flag name="VariantSign">+</flag>
                          </conditionFlags>
                          <dependencies operator="And">
                            <flagDependency flag="FeaturePack" value="+"/>
                          </dependencies>
                          <files>
                            <file source="plus/file.txt" destination="Data/file.txt"/>
                          </files>
                        </plugin>
                      </plugins>
                    </group>
                  </optionalFileGroups>
                </installStep>
              </installSteps>
            </config>
        "#;

        let cfg = parse_fomod_xml(xml).unwrap();
        let plugin = &cfg.steps[0].groups[0].plugins[0];
        assert_eq!(plugin.image_path.as_deref(), Some("images/plus.png"));
        assert_eq!(
            plugin.condition_flags,
            vec![ConditionFlag {
                name: "VariantSign".to_string(),
                value: "+".to_string(),
            }]
        );
        assert_eq!(
            plugin.dependencies,
            Some(PluginDependencies {
                operator: DependencyOperator::And,
                flags: vec![FlagDependency {
                    flag: "FeaturePack".to_string(),
                    value: "+".to_string(),
                }],
            })
        );
        assert!(cfg.steps[0].visible.is_none());
        assert!(cfg.conditional_file_installs.is_empty());
    }

    #[test]
    fn parse_fomod_xml_parses_step_visibility_and_conditional_files() {
        let xml = br#"
            <config>
              <moduleName>Example Mod</moduleName>
              <installSteps>
                <installStep name="Underwear Options">
                  <visible>
                    <flagDependency flag="bUnderwear" value="On" />
                  </visible>
                  <optionalFileGroups>
                    <group name="Color" type="SelectExactlyOne">
                      <plugins>
                        <plugin name="Black">
                          <files>
                            <folder source="16 Underwear" destination="" priority="0"/>
                          </files>
                        </plugin>
                      </plugins>
                    </group>
                  </optionalFileGroups>
                </installStep>
              </installSteps>
              <conditionalFileInstalls>
                <patterns>
                  <pattern>
                    <dependencies>
                      <flagDependency flag="bUnderwear" value="On"/>
                    </dependencies>
                    <files>
                      <folder source="22 Underwear Dark Purple" destination=""/>
                    </files>
                  </pattern>
                </patterns>
              </conditionalFileInstalls>
            </config>
        "#;

        let cfg = parse_fomod_xml(xml).unwrap();
        assert_eq!(cfg.steps.len(), 1);
        assert_eq!(
            cfg.steps[0].visible,
            Some(PluginDependencies {
                operator: DependencyOperator::And,
                flags: vec![FlagDependency {
                    flag: "bUnderwear".to_string(),
                    value: "On".to_string(),
                }],
            })
        );
        assert_eq!(cfg.conditional_file_installs.len(), 1);
        assert_eq!(
            cfg.conditional_file_installs[0].dependencies,
            PluginDependencies {
                operator: DependencyOperator::And,
                flags: vec![FlagDependency {
                    flag: "bUnderwear".to_string(),
                    value: "On".to_string(),
                }],
            }
        );
        assert_eq!(cfg.conditional_file_installs[0].files.len(), 1);
        assert_eq!(cfg.conditional_file_installs[0].files[0].source, "22 Underwear Dark Purple");
    }

    #[test]
    fn read_archive_file_bytes_finds_fomod_relative_image_path() {
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("MyMod/", b""),
                ("MyMod/fomod/", b""),
                ("MyMod/fomod/images/preview.png", b"pngdata"),
            ],
        );
        let bytes = read_archive_file_bytes(&archive, "images/preview.png").unwrap();
        assert_eq!(bytes, b"pngdata");
    }

    #[test]
    fn read_archive_file_bytes_non_zip_finds_image() {
        let tmp = tempdir();
        let Some(archive) = create_test_7z_with_image(&tmp, "mod_img.7z") else {
            return; // 7z not available
        };
        let bytes = read_archive_file_bytes(&archive, "images/preview.png").unwrap();
        assert_eq!(bytes, b"pngdata");
    }

    #[test]
    fn install_fomod_files_non_zip_installs_selected_files() {
        let tmp = tempdir();
        let Some(archive) = create_test_7z(&tmp, "mod_fomod.7z") else {
            return; // 7z not available
        };
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        // The test 7z has Data/textures/sky.dds; ask for that via a FOMOD
        // file entry with source="textures" destination="Data/textures".
        install_fomod_files(
            &archive,
            &dest,
            &[FomodFile {
                source: "textures".to_string(),
                destination: "Data/textures".to_string(),
                priority: 0,
            }],
        )
        .unwrap();

        assert!(dest.join("textures").join("sky.dds").exists());
        assert_eq!(
            std::fs::read(dest.join("textures").join("sky.dds")).unwrap(),
            b"dds",
        );
    }

    #[test]
    fn install_fomod_files_from_dir_same_result_as_zip() {
        // Verify that install_fomod_files_from_dir produces the same layout as
        // the zip-based code by comparing results for identical content.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
                ("textures/sky.dds", b"dds_data"),
            ],
        );
        let dest_zip = tmp.join("dest_zip");
        std::fs::create_dir_all(&dest_zip).unwrap();
        let files = vec![FomodFile {
            source: "textures".to_string(),
            destination: "Data/textures".to_string(),
            priority: 0,
        }];
        install_fomod_files(&archive, &dest_zip, &files).unwrap();

        // Now do the same with the dir-based function using a matching tree.
        let extracted = tmp.join("extracted");
        std::fs::create_dir_all(extracted.join("fomod")).unwrap();
        std::fs::write(extracted.join("fomod/ModuleConfig.xml"), b"<config/>").unwrap();
        std::fs::create_dir_all(extracted.join("textures")).unwrap();
        std::fs::write(extracted.join("textures/sky.dds"), b"dds_data").unwrap();

        let dest_dir = tmp.join("dest_dir");
        std::fs::create_dir_all(&dest_dir).unwrap();
        install_fomod_files_from_dir(&extracted, &dest_dir, &files).unwrap();

        assert!(dest_dir.join("textures").join("sky.dds").exists());
        assert_eq!(
            std::fs::read(dest_zip.join("textures").join("sky.dds")).unwrap(),
            std::fs::read(dest_dir.join("textures").join("sky.dds")).unwrap(),
        );
    }

    #[test]
    fn install_fomod_files_normalizes_uppercase_dirs_to_lowercase() {
        // FOMOD archives can contain mixed-case directories such as
        // "CalienteTools/" or "TEXTURES/".  After installation they should be
        // normalised to lowercase so that they do not clash with directories
        // already installed by non-FOMOD mods (which are always normalised).
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("fomod/", b""),
                ("fomod/ModuleConfig.xml", b"<config/>"),
                ("CalienteTools/BodySlide/SliderSets/CBBE.osp", b"osp_data"),
                ("TEXTURES/actors/character/cbbe.dds", b"dds_data"),
            ],
        );
        let dest = tmp.join("mod_data");
        std::fs::create_dir_all(&dest).unwrap();

        install_fomod_files(
            &archive,
            &dest,
            &[
                FomodFile {
                    source: "CalienteTools".to_string(),
                    destination: "Data/CalienteTools".to_string(),
                    priority: 0,
                },
                FomodFile {
                    source: "TEXTURES".to_string(),
                    destination: "Data/TEXTURES".to_string(),
                    priority: 0,
                },
            ],
        )
        .unwrap();
        normalize_paths_to_lowercase(&dest);

        assert!(
            dest.join("calientetools")
                .join("bodyslide")
                .join("slidersets")
                .join("cbbe.osp")
                .exists(),
            "CalienteTools directory should be normalised to calientetools"
        );
        assert!(
            !dest.join("CalienteTools").exists(),
            "original CalienteTools dir should be gone after normalisation"
        );
        assert!(
            dest.join("textures")
                .join("actors")
                .join("character")
                .join("cbbe.dds")
                .exists(),
            "TEXTURES directory should be normalised to textures"
        );
        assert!(
            !dest.join("TEXTURES").exists(),
            "original TEXTURES dir should be gone after normalisation"
        );
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CTR: AtomicU32 = AtomicU32::new(0);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("linkmm_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── normalize_paths_to_lowercase ─────────────────────────────────────────

    #[test]
    fn normalize_paths_to_lowercase_renames_uppercase_dirs() {
        let tmp = tempdir();
        std::fs::create_dir_all(tmp.join("TEXTURES")).unwrap();
        std::fs::write(tmp.join("TEXTURES/sky.dds"), b"dds").unwrap();

        normalize_paths_to_lowercase(&tmp);

        assert!(tmp.join("textures").is_dir(), "directory should be lowercased");
        assert!(tmp.join("textures/sky.dds").exists(), "file inside renamed dir should exist");
        assert!(!tmp.join("TEXTURES").exists(), "original uppercase dir should be gone");
    }

    #[test]
    fn normalize_paths_to_lowercase_merges_duplicate_dirs() {
        // Archive might produce both TEXTURES/ and textures/ – they should be merged.
        let tmp = tempdir();
        std::fs::create_dir_all(tmp.join("TEXTURES")).unwrap();
        std::fs::write(tmp.join("TEXTURES/sky.dds"), b"upper").unwrap();
        std::fs::create_dir_all(tmp.join("textures")).unwrap();
        std::fs::write(tmp.join("textures/ground.dds"), b"lower").unwrap();

        normalize_paths_to_lowercase(&tmp);

        // Both files should end up in textures/.
        assert!(tmp.join("textures/sky.dds").exists(), "sky.dds from TEXTURES should be merged");
        assert!(tmp.join("textures/ground.dds").exists(), "ground.dds from textures should remain");
        assert!(!tmp.join("TEXTURES").exists(), "uppercase variant should be removed");
    }

    #[test]
    fn normalize_paths_to_lowercase_recurses_into_subdirs() {
        let tmp = tempdir();
        std::fs::create_dir_all(tmp.join("meshes/ARMOR")).unwrap();
        std::fs::write(tmp.join("meshes/ARMOR/helm.nif"), b"nif").unwrap();

        normalize_paths_to_lowercase(&tmp);

        assert!(tmp.join("meshes/armor/helm.nif").exists());
        assert!(!tmp.join("meshes/ARMOR").exists());
    }

    #[test]
    fn install_flat_archive_normalizes_uppercase_folder_names() {
        // Archive with TEXTURES/ and meshes/ at top level (no single wrapper prefix
        // so find_common_prefix returns "") → TEXTURES/ should be renamed textures/.
        let tmp = tempdir();
        let archive = create_test_zip(
            &tmp,
            &[
                ("TEXTURES/", b""),
                ("TEXTURES/sky.dds", b"dds_data"),
                ("meshes/helm.nif", b"nif_data"),
            ],
        );
        let dest = tmp.join("mod_dir");
        std::fs::create_dir_all(&dest).unwrap();
        let data_dest = dest.join("Data");
        std::fs::create_dir_all(&data_dest).unwrap();
        extract_zip_to(&archive, &data_dest, "", &|| {}).unwrap();
        normalize_paths_to_lowercase(&data_dest);

        assert!(
            data_dest.join("textures").join("sky.dds").exists(),
            "uppercase TEXTURES should be normalised to textures"
        );
        assert!(
            !data_dest.join("TEXTURES").exists(),
            "uppercase TEXTURES dir should be gone after normalisation"
        );
    }
}
