use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CachedNode {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: SystemTime,
    pub children: Option<Vec<CachedNode>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModMetadata {
    pub mod_id: String,
    pub root_mtime: SystemTime,
    pub nodes: Vec<CachedNode>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct VfsMetadataCache {
    pub mods: HashMap<String, ModMetadata>,
}

impl VfsMetadataCache {
    pub fn load(cache_path: &Path) -> Self {
        if !cache_path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(cache_path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, cache_path: &Path) {
        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(content) = toml::to_string_pretty(self) {
            let _ = std::fs::write(cache_path, content);
        }
    }
}

pub fn scan_directory_cached(path: &Path) -> Vec<CachedNode> {
    let mut nodes = Vec::new();
    let Ok(entries) = std::fs::read_dir(path) else {
        return nodes;
    };

    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let name = entry.file_name().to_string_lossy().to_string();

        let mut node = CachedNode {
            name,
            is_dir: meta.is_dir(),
            size: meta.len(),
            mtime: meta.modified().unwrap_or(SystemTime::now()),
            children: None,
        };

        if node.is_dir {
            node.children = Some(scan_directory_cached(&entry.path()));
        }

        nodes.push(node);
    }

    nodes
}
