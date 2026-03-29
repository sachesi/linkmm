// ══════════════════════════════════════════════════════════════════════════════
// New Mod Installation System - Link-Based Deployment for Bethesda Games
// ══════════════════════════════════════════════════════════════════════════════
//
// This module implements a complete rewrite of the mod installation system
// following the Bethesda Mod Installation guidelines for linkmm.
//
// Core Principle: NEVER copy files into Data/. Always use symbolic or hard links
// to maintain a clean separation between mod storage and game deployment.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;


// ── Constants ─────────────────────────────────────────────────────────────────

/// Buffer size for file extraction operations (256 KB)
#[allow(dead_code)]
const EXTRACT_BUFFER_SIZE: usize = 256 * 1024;

/// Progress callback interval in milliseconds (50ms = ~20 Hz)
#[allow(dead_code)]
const EXTRACTION_TICK_INTERVAL_MS: u64 = 50;

/// Known Data/ subdirectories for detection heuristics
const KNOWN_DATA_SUBDIRS: &[&str] = &[
    "meshes", "textures", "scripts", "sound", "music", "shaders",
    "lodsettings", "seq", "interface", "skse", "f4se", "nvse", "obse",
    "fose", "mwse", "xnvse", "strings", "video", "facegen", "grass",
    "shadersfx", "terrain", "dialogueviews", "vis", "lightingtemplate",
    "distantlod", "lod", "trees", "fxmaster", "sky",
];

/// Known plugin file extensions
const KNOWN_PLUGIN_EXTS: &[&str] = &["esp", "esm", "esl"];

/// Known archive extensions
const KNOWN_ARCHIVE_EXTS: &[&str] = &["bsa", "ba2"];

// ── Link Type Decision ────────────────────────────────────────────────────────

/// Type of link to create when deploying files
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// Symbolic link (works across filesystems)
    Symlink,
    /// Hard link (same filesystem only, faster, no dangling risk)
    Hardlink,
}

/// Determine which link type to use based on filesystem boundaries.
///
/// Uses the device ID (`st_dev`) to detect if source and destination are on
/// the same filesystem. Hardlinks are preferred when possible because they:
/// - Are faster to create
/// - Cannot dangle (inode-based)
/// - Survive renames of the store directory
///
/// Falls back to symlinks if filesystems differ or if device check fails.
#[cfg(unix)]
pub fn determine_link_type(src: &Path, dest_dir: &Path) -> LinkKind {
    use std::os::unix::fs::MetadataExt;

    let src_dev = fs::metadata(src)
        .map(|m| m.dev())
        .unwrap_or(0);
    let dest_dev = fs::metadata(dest_dir)
        .map(|m| m.dev())
        .unwrap_or(1); // Different default to force symlink on failure

    if src_dev != 0 && src_dev == dest_dev {
        LinkKind::Hardlink
    } else {
        LinkKind::Symlink
    }
}

#[cfg(not(unix))]
pub fn determine_link_type(_src: &Path, _dest_dir: &Path) -> LinkKind {
    LinkKind::Symlink
}

// ── Path Normalization ────────────────────────────────────────────────────────

/// Normalize a path to lowercase for case-insensitive comparison.
///
/// **Critical for Linux**: The game engine is case-insensitive but the
/// filesystem is not. We must normalize all Data/-relative paths to lowercase
/// before creating links to ensure consistency.
///
/// Example: `Textures/Armor/helmet.dds` → `textures/armor/helmet.dds`
pub fn normalize_path_lowercase(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Normalize a path component for safe filesystem operations.
///
/// - Converts backslashes to forward slashes
/// - Strips leading slashes
/// - Trims trailing slashes
/// - Does NOT lowercase (use normalize_path_lowercase for that)
#[allow(dead_code)]
pub fn normalize_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_start_matches('/');
    normalized.trim_end_matches('/').to_string()
}

/// Strip "Data/" prefix from a path (case-insensitive).
///
/// Returns the path without the prefix if present, otherwise returns the
/// original path unchanged.
#[allow(dead_code)]
pub fn strip_data_prefix(path: &str) -> String {
    let normalized = normalize_path(path);
    let lower = normalized.to_lowercase();

    if lower.starts_with("data/") {
        normalized[5..].to_string()
    } else if lower == "data" {
        String::new()
    } else {
        normalized
    }
}

/// Check if a path is safe (no directory traversal).
///
/// Rejects paths containing:
/// - `..` components that would escape the target directory
/// - Absolute paths
///
/// This is critical zip-slip protection.
#[allow(dead_code)]
pub fn is_safe_relative_path(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }

    let mut depth = 0i32;
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false; // Escaped root
                }
            }
            std::path::Component::CurDir => {} // Allowed, no-op
            _ => return false, // Prefix, RootDir not allowed in relative paths
        }
    }
    true
}

// ── Archive Root Detection ────────────────────────────────────────────────────

/// Score a directory prefix as a candidate Data/ root.
///
/// Scoring weights:
/// - **+20**: Directory itself named "data" (case-insensitive)
/// - **+10**: Each known Data/ subdirectory found as direct child
/// - **+15**: Each plugin file (.esp/.esm/.esl) found as direct child
///
/// Only counts each unique child name once to prevent duplicates from
/// inflating scores.
pub fn score_as_data_root(prefix: &str, paths: &[String]) -> i32 {
    let mut score = 0i32;

    // +20 if directory itself is named "data"
    let prefix_norm = normalize_path_lowercase(prefix);
    let dir_name = prefix_norm
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");

    if dir_name == "data" {
        score += 20;
    }

    // Build search prefix with trailing slash
    let search_prefix = if prefix_norm.is_empty() {
        String::new()
    } else {
        format!("{}/", prefix_norm.trim_end_matches('/'))
    };

    let mut seen = HashSet::new();

    for path in paths {
        let path_norm = normalize_path_lowercase(path);

        // Strip candidate prefix
        let rel = if !search_prefix.is_empty() {
            match path_norm.strip_prefix(&search_prefix) {
                Some(r) => r,
                None => continue,
            }
        } else {
            &path_norm
        };

        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }

        // Get first component (immediate child)
        let first_component = rel.split('/').next().unwrap_or("");
        if first_component.is_empty() {
            continue;
        }

        // Only score each unique child once
        if !seen.insert(first_component.to_string()) {
            continue;
        }

        // Check if it's a known subdirectory
        if KNOWN_DATA_SUBDIRS.contains(&first_component) {
            score += 10;
        }

        // Check if it's a plugin file
        for ext in KNOWN_PLUGIN_EXTS {
            if first_component.ends_with(&format!(".{}", ext)) {
                score += 15;
                break;
            }
        }

        // Check for archives
        for ext in KNOWN_ARCHIVE_EXTS {
            if first_component.ends_with(&format!(".{}", ext)) {
                score += 5;
                break;
            }
        }
    }

    score
}

/// Detect the Data/ root directory within an archive.
///
/// Returns the prefix path that should be stripped during extraction.
/// Returns empty string if archive root is already the Data/ layout.
/// Returns None if detection fails and user intervention is needed.
pub fn detect_data_root(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }

    // Collect all unique top-level directories
    let mut top_level_dirs = HashSet::new();
    let mut has_root_level_data_indicators = false;

    for path in paths {
        let normalized = normalize_path_lowercase(path);
        let first_component = normalized.split('/').next().unwrap_or("");

        if first_component.is_empty() {
            continue;
        }

        top_level_dirs.insert(first_component.to_string());

        // Check if root level already looks like Data/
        if KNOWN_DATA_SUBDIRS.contains(&first_component) {
            has_root_level_data_indicators = true;
        }

        for ext in KNOWN_PLUGIN_EXTS {
            if first_component.ends_with(&format!(".{}", ext)) {
                has_root_level_data_indicators = true;
                break;
            }
        }
    }

    log::debug!(
        "[DataRoot] Scanning archive | top_level_dirs={}, total_paths={}",
        top_level_dirs.len(),
        paths.len()
    );

    // Case 1: Root level already looks like Data/ (most direct)
    if has_root_level_data_indicators {
        log::debug!("[DataRoot] Root level already has data indicators, no prefix to strip");
        return Some(String::new()); // No prefix to strip
    }

    // Case 2: Single top-level directory (common wrapper pattern)
    if top_level_dirs.len() == 1 {
        let candidate = top_level_dirs.iter().next().unwrap();
        let score = score_as_data_root(candidate, paths);

        log::debug!(
            "[DataRoot] Single wrapper directory | candidate={}, score={}",
            candidate,
            score
        );

        // Only accept if score is reasonable (at least one indicator)
        if score >= 10 {
            return Some(candidate.clone());
        }
    }

    // Case 3: Multiple top-level directories - score each
    let mut best_score = 0i32;
    let mut best_prefix = String::new();

    // Try empty prefix (root)
    let root_score = score_as_data_root("", paths);
    if root_score > 0 {
        best_score = root_score;
        best_prefix = String::new();
    }

    // Try each top-level directory
    for dir in &top_level_dirs {
        let score = score_as_data_root(dir, paths);
        log::debug!(
            "[DataRoot] Candidate scoring | prefix='{}', score={}",
            dir,
            score
        );
        if score > best_score {
            best_score = score;
            best_prefix = dir.clone();
        }
    }

    // Require minimum score threshold
    if best_score >= 10 {
        log::debug!(
            "[DataRoot] Best candidate selected | prefix='{}', score={}",
            best_prefix,
            best_score
        );
        Some(best_prefix)
    } else {
        log::debug!(
            "[DataRoot] No candidate met threshold | best_score={}, threshold=10",
            best_score
        );
        None // User must select root
    }
}

// ── Installation Strategy ─────────────────────────────────────────────────────

/// How a mod archive should be installed
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum InstallStrategy {
    /// Simple Data/ mod - extract and deploy
    SimpleData {
        /// Prefix to strip from archive paths (e.g., "ModName/" wrapper)
        strip_prefix: String,
    },
    /// FOMOD installer - user must make selections
    Fomod {
        /// Parsed FOMOD configuration
        config: FomodConfig,
    },
}

// ── FOMOD Types ───────────────────────────────────────────────────────────────

/// A single file mapping in FOMOD
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FomodFile {
    /// Source path in archive (case-insensitive match)
    pub source: String,
    /// Destination relative to Data/ directory
    pub destination: String,
    /// Priority for conflict resolution (higher wins)
    pub priority: i32,
    /// Document order for tie-breaking
    pub doc_order: usize,
}

/// FOMOD group selection type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
#[allow(clippy::enum_variant_names)]
pub enum FomodGroupType {
    SelectAll,        // All mandatory
    SelectAny,        // Zero or more
    SelectExactlyOne, // Exactly one required
    SelectAtMostOne,  // Zero or one
    SelectAtLeastOne, // One or more required
}

/// FOMOD plugin type descriptor
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FomodPluginType {
    Required,
    Optional,
    Recommended,
    NotUsable,
}

/// Flag dependency for conditional logic
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagDependency {
    pub flag: String,
    pub value: String,
}

/// Dependency operator
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyOperator {
    And,
    Or,
}

/// Dependencies for plugin visibility
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PluginDependencies {
    pub operator: DependencyOperator,
    pub flags: Vec<FlagDependency>,
}

impl PluginDependencies {
    /// Check if dependencies are satisfied given current flags
    #[allow(dead_code)]
    pub fn evaluate(&self, active_flags: &HashMap<String, String>) -> bool {
        let result = match self.operator {
            DependencyOperator::And => {
                self.flags.iter().all(|dep| {
                    let matched = active_flags.get(&dep.flag.to_lowercase())
                        == Some(&dep.value.to_lowercase());
                    log::debug!(
                        "[Dependency Evaluated] Condition: {}={} -> Result: {} | operator={:?}",
                        dep.flag,
                        dep.value,
                        matched,
                        self.operator
                    );
                    matched
                })
            }
            DependencyOperator::Or => {
                self.flags.iter().any(|dep| {
                    let matched = active_flags.get(&dep.flag.to_lowercase())
                        == Some(&dep.value.to_lowercase());
                    log::debug!(
                        "[Dependency Evaluated] Condition: {}={} -> Result: {} | operator={:?}",
                        dep.flag,
                        dep.value,
                        matched,
                        self.operator
                    );
                    matched
                })
            }
        };
        log::debug!(
            "[Dependency Group] operator={:?}, flag_count={}, overall_result={}",
            self.operator,
            self.flags.len(),
            result
        );
        result
    }
}

/// Condition flag set by plugin selection
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ConditionFlag {
    pub name: String,
    pub value: String,
}

/// A selectable plugin in a FOMOD group
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FomodPlugin {
    pub name: String,
    pub description: Option<String>,
    pub image_path: Option<String>,
    pub files: Vec<FomodFile>,
    pub plugin_type: FomodPluginType,
    pub condition_flags: Vec<ConditionFlag>,
    pub dependencies: Option<PluginDependencies>,
}

/// A group of plugins in a FOMOD install step
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FomodPluginGroup {
    pub name: String,
    pub group_type: FomodGroupType,
    pub plugins: Vec<FomodPlugin>,
}

/// A single page/step in the FOMOD installer wizard
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FomodInstallStep {
    pub name: String,
    pub visible: Option<PluginDependencies>,
    pub groups: Vec<FomodPluginGroup>,
}

/// Conditional files activated by flag patterns
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConditionalFileInstall {
    pub dependencies: PluginDependencies,
    pub files: Vec<FomodFile>,
}

/// Complete parsed FOMOD configuration
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FomodConfig {
    pub mod_name: Option<String>,
    pub required_files: Vec<FomodFile>,
    pub install_steps: Vec<FomodInstallStep>,
    pub conditional_installs: Vec<ConditionalFileInstall>,
}

// ── Conflict Resolution ───────────────────────────────────────────────────────

/// Resolve conflicts within a file installation list.
///
/// When multiple files target the same destination:
/// 1. Sort by destination path (group conflicts)
/// 2. Higher priority wins
/// 3. Later doc_order wins (tie-breaker)
/// 4. Keep only the winner for each destination
#[allow(dead_code)]
pub fn resolve_file_conflicts(mut files: Vec<FomodFile>) -> Vec<FomodFile> {
    if files.is_empty() {
        return files;
    }

    // Sort: destination (asc), priority (desc), doc_order (desc)
    files.sort_by(|a, b| {
        normalize_path_lowercase(&a.destination)
            .cmp(&normalize_path_lowercase(&b.destination))
            .then(b.priority.cmp(&a.priority))
            .then(b.doc_order.cmp(&a.doc_order))
    });

    // Keep first occurrence of each destination (= winner)
    let mut seen = HashSet::new();
    let total_before = files.len();
    files.retain(|f| {
        let dest_norm = normalize_path_lowercase(&f.destination);
        let is_new = seen.insert(dest_norm.clone());
        if !is_new {
            log::debug!(
                "[Conflict] Duplicate destination skipped | dest={}, source={}, priority={}",
                dest_norm,
                f.source,
                f.priority
            );
        }
        is_new
    });
    let conflicts_resolved = total_before - files.len();
    if conflicts_resolved > 0 {
        log::info!(
            "[Conflict] Resolved {} file conflicts, {} files remaining",
            conflicts_resolved,
            files.len()
        );
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_lowercase() {
        assert_eq!(
            normalize_path_lowercase("Textures\\Armor\\Helmet.DDS"),
            "textures/armor/helmet.dds"
        );
        assert_eq!(
            normalize_path_lowercase("Data/meshes/MyMod.NIF"),
            "data/meshes/mymod.nif"
        );
    }

    #[test]
    fn test_strip_data_prefix() {
        assert_eq!(strip_data_prefix("Data/meshes/file.nif"), "meshes/file.nif");
        assert_eq!(strip_data_prefix("data/textures/tex.dds"), "textures/tex.dds");
        assert_eq!(strip_data_prefix("meshes/file.nif"), "meshes/file.nif");
        assert_eq!(strip_data_prefix("Data"), "");
        assert_eq!(strip_data_prefix("data"), "");
    }

    #[test]
    fn test_is_safe_relative_path() {
        assert!(is_safe_relative_path(Path::new("meshes/armor/helmet.nif")));
        assert!(is_safe_relative_path(Path::new("./meshes/file.nif")));
        assert!(!is_safe_relative_path(Path::new("../../../etc/passwd")));
        assert!(!is_safe_relative_path(Path::new("/etc/passwd")));
        assert!(is_safe_relative_path(Path::new("folder/../other/file.txt"))); // depth stays >= 0
    }

    #[test]
    fn test_score_as_data_root_empty_prefix() {
        let paths = vec![
            "meshes/armor/helmet.nif".to_string(),
            "textures/armor/helmet.dds".to_string(),
            "MyMod.esp".to_string(),
        ];

        let score = score_as_data_root("", &paths);
        // meshes=10, textures=10, .esp=15 = 35
        assert_eq!(score, 35);
    }

    #[test]
    fn test_score_as_data_root_with_prefix() {
        let paths = vec![
            "MyMod/meshes/file.nif".to_string(),
            "MyMod/textures/file.dds".to_string(),
            "MyMod/MyMod.esp".to_string(),
        ];

        let score = score_as_data_root("MyMod", &paths);
        assert_eq!(score, 35); // Same as above
    }

    #[test]
    fn test_score_as_data_root_data_named_dir() {
        let paths = vec![
            "Data/meshes/file.nif".to_string(),
            "Data/MyMod.esp".to_string(),
        ];

        let score = score_as_data_root("Data", &paths);
        // "data" name bonus=20, meshes=10, .esp=15 = 45
        assert_eq!(score, 45);
    }

    #[test]
    fn test_detect_data_root_simple() {
        let paths = vec![
            "meshes/armor/helmet.nif".to_string(),
            "textures/armor/helmet.dds".to_string(),
            "MyMod.esp".to_string(),
        ];

        let root = detect_data_root(&paths);
        assert_eq!(root, Some(String::new())); // Root is already Data/
    }

    #[test]
    fn test_detect_data_root_single_wrapper() {
        let paths = vec![
            "MyMod/meshes/file.nif".to_string(),
            "MyMod/textures/file.dds".to_string(),
            "MyMod/MyMod.esp".to_string(),
        ];

        let root = detect_data_root(&paths);
        assert_eq!(root, Some("mymod".to_string())); // Strip "MyMod/"
    }

    #[test]
    fn test_detect_data_root_explicit_data_dir() {
        let paths = vec![
            "Data/meshes/file.nif".to_string(),
            "Data/MyMod.esp".to_string(),
        ];

        let root = detect_data_root(&paths);
        // "Data" directory itself has content, so strip it
        assert_eq!(root, Some("data".to_string()));
    }

    #[test]
    fn test_resolve_file_conflicts() {
        let files = vec![
            FomodFile {
                source: "a.txt".to_string(),
                destination: "file.txt".to_string(),
                priority: 0,
                doc_order: 1,
            },
            FomodFile {
                source: "b.txt".to_string(),
                destination: "file.txt".to_string(),
                priority: 10,
                doc_order: 2,
            },
            FomodFile {
                source: "c.txt".to_string(),
                destination: "other.txt".to_string(),
                priority: 0,
                doc_order: 3,
            },
        ];

        let resolved = resolve_file_conflicts(files);
        assert_eq!(resolved.len(), 2); // Two unique destinations

        // file.txt should be b.txt (higher priority)
        let file_txt = resolved.iter().find(|f| f.destination == "file.txt").unwrap();
        assert_eq!(file_txt.source, "b.txt");

        // other.txt should be c.txt
        let other_txt = resolved.iter().find(|f| f.destination == "other.txt").unwrap();
        assert_eq!(other_txt.source, "c.txt");
    }

    #[test]
    fn test_resolve_file_conflicts_doc_order_tiebreak() {
        let files = vec![
            FomodFile {
                source: "a.txt".to_string(),
                destination: "file.txt".to_string(),
                priority: 5,
                doc_order: 1,
            },
            FomodFile {
                source: "b.txt".to_string(),
                destination: "file.txt".to_string(),
                priority: 5,
                doc_order: 2,
            },
        ];

        let resolved = resolve_file_conflicts(files);
        assert_eq!(resolved.len(), 1);

        // Later doc_order wins on tie
        assert_eq!(resolved[0].source, "b.txt");
        assert_eq!(resolved[0].doc_order, 2);
    }

    #[test]
    fn test_dependency_evaluate_and() {
        let deps = PluginDependencies {
            operator: DependencyOperator::And,
            flags: vec![
                FlagDependency {
                    flag: "TEX_QUALITY".to_string(),
                    value: "4K".to_string(),
                },
                FlagDependency {
                    flag: "BODY_TYPE".to_string(),
                    value: "CBBE".to_string(),
                },
            ],
        };

        let mut active = HashMap::new();
        active.insert("tex_quality".to_string(), "4k".to_string());
        active.insert("body_type".to_string(), "cbbe".to_string());

        assert!(deps.evaluate(&active));

        // Missing one flag
        active.remove("body_type");
        assert!(!deps.evaluate(&active));
    }

    #[test]
    fn test_dependency_evaluate_or() {
        let deps = PluginDependencies {
            operator: DependencyOperator::Or,
            flags: vec![
                FlagDependency {
                    flag: "OPTION_A".to_string(),
                    value: "YES".to_string(),
                },
                FlagDependency {
                    flag: "OPTION_B".to_string(),
                    value: "YES".to_string(),
                },
            ],
        };

        let mut active = HashMap::new();
        active.insert("option_a".to_string(), "yes".to_string());

        assert!(deps.evaluate(&active)); // One is enough

        active.clear();
        assert!(!deps.evaluate(&active)); // None satisfied
    }
}
