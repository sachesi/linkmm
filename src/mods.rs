use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use crate::games::Game;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mod {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub nexus_id: Option<u32>,
    pub source_path: PathBuf,
}

impl Mod {
    pub fn new(name: impl Into<String>, source_path: PathBuf) -> Self {
        let name = name.into();
        let id = format!(
            "{}_{}",
            name.to_lowercase().replace(' ', "_"),
            source_path.to_string_lossy().len()
        );
        Self {
            id,
            name,
            version: None,
            enabled: false,
            priority: 0,
            nexus_id: None,
            source_path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModDatabase {
    pub mods: Vec<Mod>,
    pub load_order: Vec<String>,
}

impl ModDatabase {
    pub fn load(game: &Game) -> Self {
        let path = game.mods_dir().join("mods.json");
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    match serde_json::from_str::<ModDatabase>(&contents) {
                        Ok(db) => return db,
                        Err(e) => {
                            log::warn!("Failed to parse mods database: {e}, using empty database");
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read mods database: {e}");
                }
            }
        }
        Self::default()
    }

    pub fn save(&self, game: &Game) {
        let mods_dir = game.mods_dir();
        if let Err(e) = std::fs::create_dir_all(&mods_dir) {
            log::error!("Failed to create mods directory: {e}");
            return;
        }
        let path = mods_dir.join("mods.json");
        match serde_json::to_string_pretty(self) {
            Ok(contents) => {
                if let Err(e) = std::fs::write(&path, contents) {
                    log::error!("Failed to write mods database: {e}");
                }
            }
            Err(e) => {
                log::error!("Failed to serialize mods database: {e}");
            }
        }
    }

    pub fn scan_mods_dir(&mut self, game: &Game) {
        let mods_dir = game.mods_dir();
        if !mods_dir.is_dir() {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(&mods_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                // Skip entries that are the database file (shouldn't happen since we check is_dir, but be safe)
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let already_known = self.mods.iter().any(|m| m.source_path == path);
                if !already_known && !name.is_empty() {
                    let mod_entry = Mod::new(name, path);
                    self.mods.push(mod_entry);
                }
            }
        }
    }
}

pub struct ModManager;

impl ModManager {
    pub fn enable_mod(game: &Game, mod_entry: &Mod) -> Result<(), String> {
        let data_dir = &game.data_path;
        if !data_dir.is_dir() {
            std::fs::create_dir_all(data_dir)
                .map_err(|e| format!("Failed to create data directory: {e}"))?;
        }
        link_directory_contents(&mod_entry.source_path, data_dir)
    }

    pub fn disable_mod(game: &Game, mod_entry: &Mod) -> Result<(), String> {
        let data_dir = &game.data_path;
        unlink_directory_contents(&mod_entry.source_path, data_dir)
    }

    pub fn create_mod_directory(game: &Game, mod_name: &str) -> Result<PathBuf, String> {
        let dir = game.mods_dir().join(mod_name);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create mod directory: {e}"))?;
        Ok(dir)
    }
}

fn link_directory_contents(source: &Path, dest: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!("Source is not a directory: {}", source.display()));
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("Failed to create destination directory: {e}"))?;

    let entries = std::fs::read_dir(source)
        .map_err(|e| format!("Failed to read source directory: {e}"))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest.join(&file_name);

        if src_path.is_dir() {
            link_directory_contents(&src_path, &dest_path)?;
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                if dest_path.exists() || dest_path.is_symlink() {
                    // Skip if already linked or exists
                    continue;
                }
                symlink(&src_path, &dest_path)
                    .map_err(|e| format!("Failed to create symlink {:?} -> {:?}: {e}", dest_path, src_path))?;
            }
            #[cfg(not(unix))]
            {
                return Err("Symlinks are only supported on Unix systems".to_string());
            }
        }
    }
    Ok(())
}

fn unlink_directory_contents(source: &Path, dest: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Ok(());
    }
    if !dest.is_dir() {
        return Ok(());
    }

    let entries = std::fs::read_dir(source)
        .map_err(|e| format!("Failed to read source directory: {e}"))?;

    for entry in entries.flatten() {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest.join(&file_name);

        if src_path.is_dir() {
            unlink_directory_contents(&src_path, &dest_path)?;
        } else {
            #[cfg(unix)]
            {
                if dest_path.is_symlink() {
                    std::fs::remove_file(&dest_path)
                        .map_err(|e| format!("Failed to remove symlink {:?}: {e}", dest_path))?;
                }
            }
        }
    }
    Ok(())
}
