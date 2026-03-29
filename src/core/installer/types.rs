use std::collections::HashMap;

// ── Constants ─────────────────────────────────────────────────────────────────

/// FOMOD configuration directory prefix.
pub(super) const FOMOD_DIR_PREFIX: &str = "fomod/";

/// Write-buffer capacity used when extracting individual archive entries.
pub(super) const EXTRACT_BUFFER_SIZE: usize = 256 * 1024;

/// Maximum initial allocation when reading a single file from a 7z archive.
pub(super) const SINGLE_FILE_READ_CAP: usize = 64 * 1024 * 1024;

/// How often the extraction loop calls the progress `tick` callback.
pub(super) const EXTRACTION_TICK_INTERVAL_MS: u64 = 50;

/// Well-known subdirectory names inside a game's `Data/` folder.
pub(super) const KNOWN_DATA_SUBDIRS: &[&str] = &[
    "meshes",
    "textures",
    "scripts",
    "sound",
    "music",
    "shaders",
    "lodsettings",
    "seq",
    "interface",
    "skse",
    "f4se",
    "nvse",
    "obse",
    "fose",
    "mwse",
    "xnvse",
    "strings",
    "video",
    "facegen",
    "grass",
    "shadersfx",
    "terrain",
    "dialogueviews",
    "vis",
    "lightingtemplate",
    "distantlod",
    "lod",
    "trees",
    "fxmaster",
    "sky",
];

/// Plugin file extensions that strongly indicate a game data root.
pub(super) const KNOWN_PLUGIN_EXTS: &[&str] = &["esp", "esm", "esl"];

/// Known archive extensions.
pub(super) const KNOWN_ARCHIVE_EXTS: &[&str] = &["bsa", "ba2"];

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

/// A single file/folder mapping inside a FOMOD config.
#[derive(Debug, Clone)]
pub struct FomodFile {
    pub source: String,
    pub destination: String,
    pub priority: i32,
}

/// Selection type for a FOMOD plugin group.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum FomodGroupType {
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
    pub type_descriptor: FomodPluginType,
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

impl PluginDependencies {
    /// Check if dependencies are satisfied given current flags.
    ///
    /// Comparison is case-insensitive on both flag names and values.
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

/// Default selection state of a FOMOD plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum FomodPluginType {
    Required,
    Optional,
    Recommended,
    NotUsable,
}

/// A group of plugins that the user must choose from.
#[derive(Debug, Clone)]
pub struct FomodPluginGroup {
    pub name: String,
    pub group_type: FomodGroupType,
    pub plugins: Vec<FomodPlugin>,
}

/// A single install step presented in the FOMOD wizard.
#[derive(Debug, Clone)]
pub struct FomodInstallStep {
    pub name: String,
    pub visible: Option<PluginDependencies>,
    pub groups: Vec<FomodPluginGroup>,
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
    pub steps: Vec<FomodInstallStep>,
    pub conditional_file_installs: Vec<ConditionalFileInstall>,
}

// ── Link type ─────────────────────────────────────────────────────────────────

/// Type of link to create when deploying files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// Symbolic link (works across filesystems).
    Symlink,
    /// Hard link (same filesystem only, faster, no dangling risk).
    Hardlink,
}

// ── Internal types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DataArchivePlan {
    Bain { top_dirs: Vec<String> },
    ExtractToData { strip_prefix: String },
    ExtractToModRoot { strip_prefix: String },
}
