use fuser::{
    BackgroundSession, FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEntry, Request, TimeOrNow,
};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use crate::core::games::Game;
use crate::core::mods::ModDatabase;
use crate::core::vfs_cache::{CachedNode, ModMetadata, VfsMetadataCache, scan_directory_cached};

const TTL: Duration = Duration::from_secs(1);
pub const WHITEOUT_PREFIX: &str = ".linkmm_whiteout_";

// ── Node types ────────────────────────────────────────────────────────────────

enum VfsNodeKind {
    Dir {
        parent: u64,
        children: HashMap<OsString, u64>,
    },
    File {
        parent: u64,
        /// Path that can be opened without going through the FUSE mount.
        /// For real game files this is /proc/self/fd/<N>/relative; for mod
        /// files it is the absolute path under the mod's source directory.
        read_path: PathBuf,
    },
    /// A marker that this file has been deleted in the writable overlay.
    Whiteout { parent: u64 },
}

struct VfsNode {
    attr: FileAttr,
    kind: VfsNodeKind,
}

// ── Filesystem ────────────────────────────────────────────────────────────────

struct ModUnionFs {
    nodes: HashMap<u64, VfsNode>,
    /// Keeps the real game Data/ directory fd alive so we can reach its files
    /// via /proc/self/fd/<N>/... after we mount on top of it.
    _real_dir_handle: Option<std::fs::File>,
    /// When `Some`, this is a writable overlay mount for tool sessions.
    /// The upper layer is a staging directory where new/changed files are
    /// written.  The lower layer is the read-only union of game + mods.
    writable_upper: Option<PathBuf>,
    /// Flag set when the tool process exits, so we can stop accepting writes.
    session_ended: Arc<AtomicBool>,
    /// Next available inode number.
    next_ino: u64,
}

impl ModUnionFs {
    fn build(game: &Game, db: &ModDatabase) -> Result<Self, String> {
        Self::build_with_upper(game, db, None, Arc::new(AtomicBool::new(false)))
    }

    fn build_with_upper(
        game: &Game,
        db: &ModDatabase,
        writable_upper: Option<PathBuf>,
        session_ended: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let now = SystemTime::now();

        // Open game Data/ *before* mounting so we can still read its files
        // afterward via the /proc/self/fd/<N> path (avoids FUSE re-entry).
        let (real_dir_handle, game_proc_path) = if game.data_path.is_dir() {
            let f = std::fs::File::open(&game.data_path)
                .map_err(|e| format!("Cannot open game data dir: {e}"))?;
            let fd = f.as_raw_fd();
            let p = PathBuf::from(format!("/proc/self/fd/{fd}"));
            (Some(f), p)
        } else {
            (None, PathBuf::new())
        };

        let mut fs = ModUnionFs {
            nodes: HashMap::new(),
            _real_dir_handle: real_dir_handle,
            writable_upper,
            session_ended,
            next_ino: 2,
        };

        // Root inode = 1 (required by FUSE spec)
        fs.nodes.insert(
            1,
            VfsNode {
                attr: dir_attr(1, uid, gid, now),
                kind: VfsNodeKind::Dir {
                    parent: 1,
                    children: HashMap::new(),
                },
            },
        );

        // Layer 0: real game Data/ (lowest priority — every mod overrides it)
        if !game_proc_path.as_os_str().is_empty() {
            fs.overlay(&game_proc_path, 1, uid, gid, now)?;
        }

        let cache_path = game.config_dir().join("vfs_metadata.toml");
        let mut cache = VfsMetadataCache::load(&cache_path);
        let mut cache_updated = false;

        // Layers 1‥N: enabled mods, ascending priority (highest number wins)
        let mut mods: Vec<_> = db.mods.iter().filter(|m| m.enabled).collect();
        mods.sort_by_key(|m| m.priority);
        for m in mods {
            let mod_data = m.source_path.join("Data");
            let root = if mod_data.is_dir() {
                mod_data
            } else {
                m.source_path.clone()
            };
            if !root.is_dir() {
                continue;
            }

            let root_mtime = root.metadata().and_then(|m| m.modified()).unwrap_or(now);

            let use_cache = if let Some(meta) = cache.mods.get(&m.id) {
                meta.root_mtime == root_mtime
            } else {
                false
            };

            if use_cache {
                let nodes = &cache.mods.get(&m.id).unwrap().nodes;
                fs.overlay_cached(nodes, &root, 1, uid, gid, now)?;
            } else {
                log::debug!("Cache miss for mod {}, scanning {}", m.id, root.display());
                let nodes = scan_directory_cached(&root);
                fs.overlay_cached(&nodes, &root, 1, uid, gid, now)?;
                cache.mods.insert(
                    m.id.clone(),
                    ModMetadata {
                        mod_id: m.id.clone(),
                        root_mtime,
                        nodes,
                    },
                );
                cache_updated = true;
            }
        }

        if cache_updated {
            cache.save(&cache_path);
        }

        // Upper layer (writable staging) — highest priority, overrides everything
        let upper_path = fs.writable_upper.clone();
        if let Some(ref upper) = upper_path {
            if upper.is_dir() {
                fs.overlay(upper, 1, uid, gid, now)?;
            }
        }

        log::debug!(
            "VFS built: {} inodes for {} enabled mods, writable_upper={}",
            fs.nodes.len(),
            db.mods.iter().filter(|m| m.enabled).count(),
            fs.writable_upper
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "none".to_string())
        );
        Ok(fs)
    }

    fn overlay(
        &mut self,
        src_dir: &Path,
        parent_ino: u64,
        uid: u32,
        gid: u32,
        now: SystemTime,
    ) -> Result<(), String> {
        let entries: Vec<_> = std::fs::read_dir(src_dir)
            .map_err(|e| format!("Cannot read {}: {e}", src_dir.display()))?
            .flatten()
            .collect();

        for entry in entries {
            let src = entry.path();
            if src.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Handle whiteouts
            if name_str.starts_with(WHITEOUT_PREFIX) {
                let target_name = &name_str[WHITEOUT_PREFIX.len()..];
                let target_name_lc = target_name.to_lowercase();
                if let Some(ino) = self.child_ino_ci(parent_ino, &target_name_lc) {
                    // Mark as whiteout in VFS
                    if let Some(node) = self.nodes.get_mut(&ino) {
                        node.kind = VfsNodeKind::Whiteout { parent: parent_ino };
                    }
                }
                continue;
            }

            let Ok(meta) = std::fs::metadata(&src) else {
                continue;
            };
            let name_lc = name_str.to_lowercase();
            let existing = self.child_ino_ci(parent_ino, &name_lc);

            if meta.is_dir() {
                let child_ino = match existing {
                    Some(ino) => ino,
                    None => {
                        let ino = self.next_ino;
                        self.next_ino += 1;
                        self.nodes.insert(
                            ino,
                            VfsNode {
                                attr: dir_attr(ino, uid, gid, now),
                                kind: VfsNodeKind::Dir {
                                    parent: parent_ino,
                                    children: HashMap::new(),
                                },
                            },
                        );
                        self.insert_child(parent_ino, name.clone(), ino);
                        ino
                    }
                };
                self.overlay(&src, child_ino, uid, gid, now)?;
            } else if meta.is_file() {
                let size = meta.len();
                let mtime = meta.modified().unwrap_or(now);
                match existing {
                    Some(ino) => {
                        if let Some(node) = self.nodes.get_mut(&ino) {
                            node.attr.size = size;
                            node.attr.mtime = mtime;
                            node.attr.blocks = blocks(size);
                            node.kind = VfsNodeKind::File {
                                parent: parent_ino,
                                read_path: src,
                            };
                        }
                    }
                    None => {
                        let ino = self.next_ino;
                        self.next_ino += 1;
                        self.nodes.insert(
                            ino,
                            VfsNode {
                                attr: file_attr(ino, size, mtime, uid, gid, now),
                                kind: VfsNodeKind::File {
                                    parent: parent_ino,
                                    read_path: src,
                                },
                            },
                        );
                        self.insert_child(parent_ino, name.clone(), ino);
                    }
                }
            }
        }
        Ok(())
    }

    /// Recursively apply cached metadata nodes to the VFS.
    fn overlay_cached(
        &mut self,
        nodes: &[CachedNode],
        base_path: &Path,
        parent_ino: u64,
        uid: u32,
        gid: u32,
        now: SystemTime,
    ) -> Result<(), String> {
        for node in nodes {
            let name = OsString::from(&node.name);
            let name_lc = node.name.to_lowercase();
            let existing = self.child_ino_ci(parent_ino, &name_lc);
            let src = base_path.join(&name);

            if node.is_dir {
                let child_ino = match existing {
                    Some(ino) => ino,
                    None => {
                        let ino = self.next_ino;
                        self.next_ino += 1;
                        self.nodes.insert(
                            ino,
                            VfsNode {
                                attr: dir_attr(ino, uid, gid, now),
                                kind: VfsNodeKind::Dir {
                                    parent: parent_ino,
                                    children: HashMap::new(),
                                },
                            },
                        );
                        self.insert_child(parent_ino, name, ino);
                        ino
                    }
                };
                if let Some(ref children) = node.children {
                    self.overlay_cached(children, &src, child_ino, uid, gid, now)?;
                }
            } else {
                match existing {
                    Some(ino) => {
                        if let Some(vfs_node) = self.nodes.get_mut(&ino) {
                            vfs_node.attr.size = node.size;
                            vfs_node.attr.mtime = node.mtime;
                            vfs_node.attr.blocks = blocks(node.size);
                            vfs_node.kind = VfsNodeKind::File {
                                parent: parent_ino,
                                read_path: src,
                            };
                        }
                    }
                    None => {
                        let ino = self.next_ino;
                        self.next_ino += 1;
                        self.nodes.insert(
                            ino,
                            VfsNode {
                                attr: file_attr(ino, node.size, node.mtime, uid, gid, now),
                                kind: VfsNodeKind::File {
                                    parent: parent_ino,
                                    read_path: src,
                                },
                            },
                        );
                        self.insert_child(parent_ino, name, ino);
                    }
                }
            }
        }
        Ok(())
    }

    fn child_ino_ci(&self, parent: u64, name_lc: &str) -> Option<u64> {
        let node = self.nodes.get(&parent)?;
        if let VfsNodeKind::Dir { children, .. } = &node.kind {
            children
                .iter()
                .find(|(k, _)| k.to_string_lossy().to_lowercase() == name_lc)
                .map(|(_, &ino)| ino)
        } else {
            None
        }
    }

    fn insert_child(&mut self, parent: u64, name: OsString, child: u64) {
        if let Some(VfsNode {
            kind: VfsNodeKind::Dir { children, .. },
            ..
        }) = self.nodes.get_mut(&parent)
        {
            children.insert(name, child);
        }
    }

    /// Copy a file from the lower layer to the upper layer (copy-on-write).
    /// Returns the path in the upper layer.
    fn cow_file(&mut self, ino: u64) -> Result<PathBuf, String> {
        let upper = self
            .writable_upper
            .as_ref()
            .ok_or_else(|| "No writable upper layer".to_string())?
            .clone();

        let node = self
            .nodes
            .get(&ino)
            .ok_or_else(|| format!("Inode {ino} not found"))?;
        let VfsNodeKind::File { read_path, .. } = &node.kind else {
            return Err("Not a file".to_string());
        };
        let read_path = read_path.clone();

        // Build the relative path from the mount root (inode 1) to this file.
        let rel = self
            .path_for_ino(ino)
            .ok_or_else(|| format!("Cannot resolve path for inode {ino}"))?;
        let dest = upper.join(&rel);

        if dest.exists() {
            // Already copied to upper layer (or was newly created there)
            return Ok(dest);
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create upper dir {}: {e}", parent.display()))?;
        }

        std::fs::copy(&read_path, &dest).map_err(|e| {
            format!(
                "Failed to copy {} to {}: {e}",
                read_path.display(),
                dest.display()
            )
        })?;

        // Update node to point to the new writable path
        if let Some(node) = self.nodes.get_mut(&ino) {
            if let VfsNodeKind::File {
                ref mut read_path, ..
            } = node.kind
            {
                *read_path = dest.clone();
            }
        }

        Ok(dest)
    }

    /// Resolve the relative path from root (inode 1) to the given inode.
    fn path_for_ino(&self, ino: u64) -> Option<PathBuf> {
        if ino == 1 {
            return Some(PathBuf::new());
        }
        let node = self.nodes.get(&ino)?;
        let parent = match &node.kind {
            VfsNodeKind::Dir { parent, .. } => *parent,
            VfsNodeKind::File { parent, .. } => *parent,
            VfsNodeKind::Whiteout { parent } => *parent,
        };

        let mut path = self.path_for_ino(parent)?;

        // Find the name of this inode in its parent's children
        let parent_node = self.nodes.get(&parent)?;
        if let VfsNodeKind::Dir { children, .. } = &parent_node.kind {
            for (name, &child_ino) in children {
                if child_ino == ino {
                    path.push(name);
                    return Some(path);
                }
            }
        }
        None
    }
}

impl Filesystem for ModUnionFs {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_lc = name.to_string_lossy().to_lowercase();
        let Some(ino) = self.child_ino_ci(parent, &name_lc) else {
            reply.error(libc::ENOENT);
            return;
        };
        match self.nodes.get(&ino) {
            Some(node) => {
                if matches!(node.kind, VfsNodeKind::Whiteout { .. }) {
                    reply.error(libc::ENOENT);
                } else {
                    reply.entry(&TTL, &node.attr, 0)
                }
            }
            None => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        match self.nodes.get(&ino) {
            Some(node) => {
                if matches!(node.kind, VfsNodeKind::Whiteout { .. }) {
                    reply.error(libc::ENOENT);
                } else {
                    reply.attr(&TTL, &node.attr)
                }
            }
            None => reply.error(libc::ENOENT),
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let Some(node) = self.nodes.get(&ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        match &node.kind {
            VfsNodeKind::File { read_path, .. } => {
                let path = read_path.clone();
                match std::fs::File::open(&path) {
                    Ok(mut f) => {
                        let mut buf = vec![0u8; size as usize];
                        if let Err(e) = f.seek(SeekFrom::Start(offset as u64)) {
                            log::error!("VFS seek {}: {e}", path.display());
                            reply.error(libc::EIO);
                            return;
                        }
                        match f.read(&mut buf) {
                            Ok(n) => reply.data(&buf[..n]),
                            Err(e) => {
                                log::error!("VFS read {}: {e}", path.display());
                                reply.error(libc::EIO);
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("VFS open {}: {e}", path.display());
                        reply.error(libc::EIO);
                    }
                }
            }
            VfsNodeKind::Whiteout { .. } => {
                reply.error(libc::ENOENT);
            }
            _ => {
                reply.error(libc::EISDIR);
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        // Clone children first to avoid double-borrow of self.nodes.
        let (children, parent_ino) = {
            let Some(node) = self.nodes.get(&ino) else {
                reply.error(libc::ENOENT);
                return;
            };
            match &node.kind {
                VfsNodeKind::Dir { children, parent } => (children.clone(), *parent),
                _ => {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            }
        };

        let mut entries: Vec<(u64, FileType, OsString)> = vec![
            (ino, FileType::Directory, ".".into()),
            (parent_ino, FileType::Directory, "..".into()),
        ];
        for (name, &child_ino) in &children {
            if let Some(node) = self.nodes.get(&child_ino) {
                if matches!(node.kind, VfsNodeKind::Whiteout { .. }) {
                    continue;
                }
                let kind = match &node.kind {
                    VfsNodeKind::Dir { .. } => FileType::Directory,
                    _ => FileType::RegularFile,
                };
                entries.push((child_ino, kind, name.clone()));
            }
        }

        for (i, (entry_ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*entry_ino, (i + 1) as i64, *kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn open(&mut self, _req: &Request<'_>, _ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        reply.opened(0, 0);
    }

    fn opendir(&mut self, _req: &Request<'_>, _ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        reply.opened(0, 0);
    }

    // ── Write operations (only when writable_upper is set) ──────────────────

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: fuser::ReplyWrite,
    ) {
        if self.session_ended.load(Ordering::SeqCst) {
            reply.error(libc::EIO);
            return;
        }

        if self.writable_upper.is_none() {
            reply.error(libc::EROFS);
            return;
        }

        // Copy-on-write: ensure the file exists in the upper layer
        let dest = match self.cow_file(ino) {
            Ok(p) => p,
            Err(e) => {
                log::error!("VFS write cow_file failed: {e}");
                reply.error(libc::EIO);
                return;
            }
        };

        match std::fs::OpenOptions::new().write(true).open(&dest) {
            Ok(mut f) => {
                use std::io::Write;
                if let Err(e) = f.seek(SeekFrom::Start(offset as u64)) {
                    log::error!("VFS write seek {}: {e}", dest.display());
                    reply.error(libc::EIO);
                    return;
                }
                match f.write(data) {
                    Ok(n) => {
                        // Update inode attributes
                        if let Some(node) = self.nodes.get_mut(&ino) {
                            if let Ok(meta) = std::fs::metadata(&dest) {
                                node.attr.size = meta.len();
                                node.attr.mtime = meta.modified().unwrap_or(SystemTime::now());
                                node.attr.blocks = blocks(meta.len());
                            }
                        }
                        reply.written(n as u32);
                    }
                    Err(e) => {
                        log::error!("VFS write {}: {e}", dest.display());
                        reply.error(libc::EIO);
                    }
                }
            }
            Err(e) => {
                log::error!("VFS open for write {}: {e}", dest.display());
                reply.error(libc::EIO);
            }
        }
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        if self.session_ended.load(Ordering::SeqCst) {
            reply.error(libc::EIO);
            return;
        }

        let upper = match &self.writable_upper {
            Some(u) => u.clone(),
            None => {
                reply.error(libc::EROFS);
                return;
            }
        };

        // Resolve parent directory path in upper layer
        let parent_path = match self.path_for_ino(parent) {
            Some(p) => upper.join(p),
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let dest = parent_path.join(name);
        if let Some(parent_dir) = dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent_dir) {
                log::error!("VFS create mkdir {}: {e}", parent_dir.display());
                reply.error(libc::EIO);
                return;
            }
        }

        match std::fs::File::create(&dest) {
            Ok(_) => {
                let uid = unsafe { libc::getuid() };
                let gid = unsafe { libc::getgid() };
                let now = SystemTime::now();
                let ino = self.next_ino;
                self.next_ino += 1;
                let attr = file_attr(ino, 0, now, uid, gid, now);
                self.nodes.insert(
                    ino,
                    VfsNode {
                        attr: attr.clone(),
                        kind: VfsNodeKind::File {
                            parent,
                            read_path: dest,
                        },
                    },
                );
                self.insert_child(parent, name.to_os_string(), ino);
                reply.created(&TTL, &attr, 0, 0, 0);
            }
            Err(e) => {
                log::error!("VFS create {}: {e}", dest.display());
                reply.error(libc::EIO);
            }
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        if self.session_ended.load(Ordering::SeqCst) {
            reply.error(libc::EIO);
            return;
        }

        let upper = match &self.writable_upper {
            Some(u) => u.clone(),
            None => {
                reply.error(libc::EROFS);
                return;
            }
        };

        let parent_path = match self.path_for_ino(parent) {
            Some(p) => upper.join(p),
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let dest = parent_path.join(name);
        match std::fs::create_dir(&dest) {
            Ok(()) => {
                let uid = unsafe { libc::getuid() };
                let gid = unsafe { libc::getgid() };
                let now = SystemTime::now();
                let ino = self.next_ino;
                self.next_ino += 1;
                let attr = dir_attr(ino, uid, gid, now);
                self.nodes.insert(
                    ino,
                    VfsNode {
                        attr: attr.clone(),
                        kind: VfsNodeKind::Dir {
                            parent,
                            children: HashMap::new(),
                        },
                    },
                );
                self.insert_child(parent, name.to_os_string(), ino);
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => {
                log::error!("VFS mkdir {}: {e}", dest.display());
                reply.error(libc::EIO);
            }
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        if self.session_ended.load(Ordering::SeqCst) {
            reply.error(libc::EIO);
            return;
        }

        let upper = match &self.writable_upper {
            Some(u) => u.clone(),
            None => {
                reply.error(libc::EROFS);
                return;
            }
        };

        let name_lc = name.to_string_lossy().to_lowercase();
        let Some(ino) = self.child_ino_ci(parent, &name_lc) else {
            reply.error(libc::ENOENT);
            return;
        };

        let parent_path = match self.path_for_ino(parent) {
            Some(p) => upper.join(p),
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let dest = parent_path.join(name);

        // If the file exists in the upper layer, delete it.
        if dest.exists() {
            if let Err(e) = std::fs::remove_file(&dest) {
                log::error!("VFS unlink {}: {e}", dest.display());
                reply.error(libc::EIO);
                return;
            }
        }

        // Check if this file exists in any lower layer.
        // If it does, we need to create a whiteout marker in the upper layer.
        // If it only existed in the upper layer (newly created), we just remove it from VFS.
        let in_lower = match self.nodes.get(&ino) {
            Some(VfsNode {
                kind: VfsNodeKind::File { read_path, .. },
                ..
            }) => !read_path.starts_with(&upper),
            _ => false,
        };

        if in_lower {
            // Create whiteout marker file
            let whiteout_path =
                parent_path.join(format!("{}{}", WHITEOUT_PREFIX, name.to_string_lossy()));
            if let Some(p) = whiteout_path.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            if let Err(e) = std::fs::File::create(&whiteout_path) {
                log::error!("VFS unlink whiteout {}: {e}", whiteout_path.display());
                reply.error(libc::EIO);
                return;
            }
            // Update node to be a whiteout
            if let Some(node) = self.nodes.get_mut(&ino) {
                node.kind = VfsNodeKind::Whiteout { parent };
            }
        } else {
            // Remove from VFS entirely
            self.nodes.remove(&ino);
            if let Some(VfsNode {
                kind: VfsNodeKind::Dir { children, .. },
                ..
            }) = self.nodes.get_mut(&parent)
            {
                children.retain(|k, _| k.to_string_lossy().to_lowercase() != name_lc);
            }
        }

        reply.ok();
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
        if self.session_ended.load(Ordering::SeqCst) {
            reply.error(libc::EIO);
            return;
        }

        let upper = match &self.writable_upper {
            Some(u) => u.clone(),
            None => {
                reply.error(libc::EROFS);
                return;
            }
        };

        let name_lc = name.to_string_lossy().to_lowercase();
        let Some(ino) = self.child_ino_ci(parent, &name_lc) else {
            reply.error(libc::ENOENT);
            return;
        };

        // If it is a file from lower layer, we must CoW it first so it exists in upper
        let is_dir = matches!(
            self.nodes.get(&ino),
            Some(VfsNode {
                kind: VfsNodeKind::Dir { .. },
                ..
            })
        );
        if !is_dir {
            if let Err(e) = self.cow_file(ino) {
                log::error!("VFS rename cow_file failed: {e}");
                reply.error(libc::EIO);
                return;
            }
        }

        let old_rel = match self.path_for_ino(ino) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let old_path = upper.join(&old_rel);

        let new_parent_rel = match self.path_for_ino(newparent) {
            Some(p) => p,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let new_path = upper.join(new_parent_rel).join(newname);

        if let Some(p) = new_path.parent() {
            let _ = std::fs::create_dir_all(p);
        }

        match std::fs::rename(&old_path, &new_path) {
            Ok(()) => {
                // Update VFS state
                if let Some(VfsNode { kind, .. }) = self.nodes.get_mut(&ino) {
                    match kind {
                        VfsNodeKind::Dir { parent, .. } => *parent = newparent,
                        VfsNodeKind::File {
                            parent, read_path, ..
                        } => {
                            *parent = newparent;
                            *read_path = new_path;
                        }
                        VfsNodeKind::Whiteout { parent } => *parent = newparent,
                    }
                }

                // Remove from old parent children
                if let Some(VfsNode {
                    kind: VfsNodeKind::Dir { children, .. },
                    ..
                }) = self.nodes.get_mut(&parent)
                {
                    children.retain(|k, _| k.to_string_lossy().to_lowercase() != name_lc);
                }

                // Add to new parent children
                self.insert_child(newparent, newname.to_os_string(), ino);

                reply.ok();
            }
            Err(e) => {
                log::error!(
                    "VFS rename {} -> {}: {e}",
                    old_path.display(),
                    new_path.display()
                );
                reply.error(libc::EIO);
            }
        }
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: fuser::ReplyAttr,
    ) {
        if self.session_ended.load(Ordering::SeqCst) {
            reply.error(libc::EIO);
            return;
        }

        if self.writable_upper.is_none() && (size.is_some() || mtime.is_some()) {
            reply.error(libc::EROFS);
            return;
        }

        // If we are modifying attributes that affect the file on disk (size, mtime),
        // we must ensure the file exists in the upper layer via CoW.
        if size.is_some() || mtime.is_some() {
            let dest = match self.cow_file(ino) {
                Ok(p) => p,
                Err(e) => {
                    log::error!("VFS setattr cow_file failed: {e}");
                    reply.error(libc::EIO);
                    return;
                }
            };

            if let Some(new_size) = size {
                if let Err(e) = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&dest)
                    .and_then(|f| f.set_len(new_size))
                {
                    log::error!(
                        "VFS setattr truncate {} to {}: {e}",
                        dest.display(),
                        new_size
                    );
                    reply.error(libc::EIO);
                    return;
                }
            }

            if let Some(new_mtime) = mtime {
                let time = match new_mtime {
                    TimeOrNow::Now => SystemTime::now(),
                    TimeOrNow::SpecificTime(t) => t,
                };
                // Note: filetime or similar could be used for more precision, but std::fs::set_modified works too
                if let Err(e) = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&dest)
                    .and_then(|f| f.set_modified(time))
                {
                    log::warn!("VFS setattr set_modified {}: {e}", dest.display());
                }
            }

            // Sync node attributes from disk
            if let Some(node) = self.nodes.get_mut(&ino) {
                if let Ok(meta) = std::fs::metadata(&dest) {
                    node.attr.size = meta.len();
                    node.attr.mtime = meta.modified().unwrap_or(SystemTime::now());
                    node.attr.blocks = blocks(meta.len());
                }
            }
        }

        match self.nodes.get(&ino) {
            Some(node) => reply.attr(&TTL, &node.attr),
            None => reply.error(libc::ENOENT),
        }
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: fuser::ReplyStatfs) {
        let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
        let res = if let Some(upper) = &self.writable_upper {
            let path = std::ffi::CString::new(upper.as_os_str().as_bytes()).unwrap();
            unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) }
        } else if let Some(ref f) = self._real_dir_handle {
            unsafe { libc::fstatfs(f.as_raw_fd(), stats.as_mut_ptr()) }
        } else {
            -1
        };

        if res == 0 {
            let stats = unsafe { stats.assume_init() };
            reply.statfs(
                stats.f_blocks as u64,
                stats.f_bfree as u64,
                stats.f_bavail as u64,
                stats.f_files as u64,
                stats.f_ffree as u64,
                stats.f_bsize as u32,
                255,
                stats.f_frsize as u32,
            );
        } else {
            // Fallback to mock values if statfs fails
            reply.statfs(
                1_000_000, // blocks
                500_000,   // bfree
                500_000,   // bavail
                100_000,   // files
                50_000,    // ffree
                4096,      // bsize
                255,       // namelen
                4096,      // frsize
            );
        }
    }
}

// ── Attribute helpers ─────────────────────────────────────────────────────────

fn dir_attr(ino: u64, uid: u32, gid: u32, now: SystemTime) -> FileAttr {
    FileAttr {
        ino,
        size: 0,
        blocks: 0,
        atime: now,
        mtime: now,
        ctime: now,
        crtime: now,
        kind: FileType::Directory,
        perm: 0o555,
        nlink: 2,
        uid,
        gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn file_attr(
    ino: u64,
    size: u64,
    mtime: SystemTime,
    uid: u32,
    gid: u32,
    now: SystemTime,
) -> FileAttr {
    FileAttr {
        ino,
        size,
        blocks: blocks(size),
        atime: now,
        mtime,
        ctime: mtime,
        crtime: mtime,
        kind: FileType::RegularFile,
        perm: 0o444,
        nlink: 1,
        uid,
        gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn blocks(size: u64) -> u64 {
    (size + 511) / 512
}

// ── Public mount API ──────────────────────────────────────────────────────────

pub struct MountHandle {
    _session: BackgroundSession,
    pub mountpoint: PathBuf,
    /// When `Some`, this is a writable overlay mount for tool sessions.
    /// The upper layer is a staging directory where new/changed files are
    /// written.  The lower layer is the read-only union of game + mods.
    pub writable_upper: Option<PathBuf>,
    /// Flag set when the tool process exits, so we can stop accepting writes.
    pub session_ended: Arc<AtomicBool>,
}

/// Mount a read-only union VFS of all enabled mods at `game.data_path`.
///
/// The real game Data/ directory is included as the lowest-priority layer;
/// enabled mods (in ascending priority order) are layered on top.
/// The mount is automatically torn down when the returned handle is dropped.
pub fn mount_mod_vfs(game: &Game, db: &ModDatabase) -> Result<MountHandle, String> {
    let mountpoint = game.data_path.clone();

    if !mountpoint.exists() {
        std::fs::create_dir_all(&mountpoint)
            .map_err(|e| format!("Cannot create game Data dir: {e}"))?;
    }

    let fs = ModUnionFs::build(game, db)?;

    let options = &[
        MountOption::RO,
        MountOption::AllowOther,
        MountOption::DefaultPermissions,
        MountOption::FSName("linkmm".to_string()),
        MountOption::NoDev,
        MountOption::NoSuid,
        MountOption::NoExec,
        MountOption::CUSTOM("nonempty".to_string()),
    ];

    let path_env = std::env::var("PATH").unwrap_or_default();
    log::debug!("VFS Mount Environment PATH: {}", path_env);

    let fusermount_check = std::process::Command::new("which")
        .arg("fusermount3")
        .output();
    match fusermount_check {
        Ok(out) if out.status.success() => log::debug!(
            "VFS found fusermount3 at: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        ),
        _ => log::warn!("VFS COULD NOT FIND fusermount3 in PATH!"),
    }

    log::info!("Mounting VFS (RO) at {}", mountpoint.display());
    let mountpoint = mountpoint.canonicalize().unwrap_or(mountpoint);

    let session = fuser::spawn_mount2(fs, &mountpoint, options)
        .map_err(|e| format!("Failed to mount mod VFS at {}: {e}", mountpoint.display()))?;

    log::info!("Mounted mod VFS at {}", mountpoint.display());
    Ok(MountHandle {
        _session: session,
        mountpoint,
        writable_upper: None,
        session_ended: Arc::new(AtomicBool::new(false)),
    })
}

/// Mount a writable overlay VFS for tool sessions.
///
/// The lower layer is the read-only union of game Data/ + enabled mods.
/// The upper layer is a fresh staging directory (`mods_dir/tool_scratch/<tool_id>/`)
/// where the tool can write new/changed files.
///
/// The mount is automatically torn down when the returned handle is dropped.
pub fn mount_tool_vfs(game: &Game, db: &ModDatabase, tool_id: &str) -> Result<MountHandle, String> {
    let mountpoint = game.data_path.clone();

    if !mountpoint.exists() {
        std::fs::create_dir_all(&mountpoint)
            .map_err(|e| format!("Cannot create game Data dir: {e}"))?;
    }

    // Create staging directory for this tool session
    let scratch_dir = game.mods_dir().join("tool_scratch").join(tool_id);
    std::fs::create_dir_all(&scratch_dir)
        .map_err(|e| format!("Failed to create tool scratch dir: {e}"))?;

    let session_ended = Arc::new(AtomicBool::new(false));

    let fs = ModUnionFs::build_with_upper(
        game,
        db,
        Some(scratch_dir.clone()),
        Arc::clone(&session_ended),
    )?;

    let options = &[
        MountOption::AllowOther,
        MountOption::DefaultPermissions,
        MountOption::FSName("linkmm-tool".to_string()),
        MountOption::NoDev,
        MountOption::NoSuid,
        MountOption::NoExec,
        MountOption::CUSTOM("nonempty".to_string()),
    ];

    log::info!("Mounting VFS (RW) at {}", mountpoint.display());
    if !mountpoint.exists() {
        return Err(format!(
            "Mountpoint does not exist: {}",
            mountpoint.display()
        ));
    }

    let session = fuser::spawn_mount2(fs, &mountpoint, options)
        .map_err(|e| format!("Failed to mount tool VFS at {}: {e}", mountpoint.display()))?;

    log::info!(
        "Mounted tool VFS at {} (writable upper: {})",
        mountpoint.display(),
        scratch_dir.display()
    );
    Ok(MountHandle {
        _session: session,
        mountpoint,
        writable_upper: Some(scratch_dir),
        session_ended,
    })
}

impl Drop for MountHandle {
    fn drop(&mut self) {
        log::info!("Unmounting VFS at {}", self.mountpoint.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::games::{Game, GameKind, GameLauncherSource};
    use crate::core::mods::{Mod, ModDatabase};
    use tempfile::TempDir;

    fn test_game(tmp: &TempDir) -> Game {
        let root = tmp.path().join("game");
        let data = root.join("Data");
        std::fs::create_dir_all(&data).unwrap();
        Game {
            id: "vfs_test".to_string(),
            name: "Test".to_string(),
            kind: GameKind::SkyrimSE,
            launcher_source: GameLauncherSource::NonSteamUmu,
            steam_app_id: None,
            root_path: root,
            data_path: data,
            mods_base_dir: Some(tmp.path().join("mods")),
            umu_config: None,
        }
    }

    fn add_mod(db: &mut ModDatabase, source: &Path, name: &str, priority: i32) {
        let mut m = Mod::new(name, source.to_path_buf());
        m.enabled = true;
        m.priority = priority;
        db.mods.push(m);
    }

    #[test]
    fn vfs_builds_without_panic_for_empty_mod_list() {
        let tmp = TempDir::new().unwrap();
        let game = test_game(&tmp);
        std::fs::write(game.data_path.join("Skyrim.esm"), b"esm").unwrap();
        let db = ModDatabase::default();
        let fs = ModUnionFs::build(&game, &db).unwrap();
        // Root + Skyrim.esm = 2 inodes
        assert!(fs.nodes.len() >= 2);
    }

    #[test]
    fn higher_priority_mod_wins_file_conflict() {
        let tmp = TempDir::new().unwrap();
        let game = test_game(&tmp);

        // Base game file
        std::fs::write(game.data_path.join("textures/tex.dds"), b"game").unwrap_or_default();
        std::fs::create_dir_all(game.data_path.join("textures")).unwrap();
        std::fs::write(game.data_path.join("textures/tex.dds"), b"game").unwrap();

        // Low-priority mod
        let mod_low = tmp.path().join("mod_low/Data/textures");
        std::fs::create_dir_all(&mod_low).unwrap();
        std::fs::write(mod_low.join("tex.dds"), b"low").unwrap();

        // High-priority mod
        let mod_high = tmp.path().join("mod_high/Data/textures");
        std::fs::create_dir_all(&mod_high).unwrap();
        std::fs::write(mod_high.join("tex.dds"), b"high").unwrap();

        let mut db = ModDatabase::default();
        add_mod(&mut db, &tmp.path().join("mod_low"), "LowMod", 0);
        add_mod(&mut db, &tmp.path().join("mod_high"), "HighMod", 1);

        let fs = ModUnionFs::build(&game, &db).unwrap();

        // Find the textures dir inode
        let textures_ino = fs.child_ino_ci(1, "textures").expect("textures dir in VFS");
        let tex_ino = fs
            .child_ino_ci(textures_ino, "tex.dds")
            .expect("tex.dds in VFS");
        let node = fs.nodes.get(&tex_ino).unwrap();
        if let VfsNodeKind::File { read_path, .. } = &node.kind {
            let data = std::fs::read(read_path).unwrap();
            assert_eq!(data, b"high", "high-priority mod should win");
        } else {
            panic!("expected File node");
        }
    }

    #[test]
    fn directories_are_merged_not_replaced() {
        let tmp = TempDir::new().unwrap();
        let game = test_game(&tmp);

        let mod_a = tmp.path().join("mod_a/Data/meshes");
        std::fs::create_dir_all(&mod_a).unwrap();
        std::fs::write(mod_a.join("a.nif"), b"a").unwrap();

        let mod_b = tmp.path().join("mod_b/Data/meshes");
        std::fs::create_dir_all(&mod_b).unwrap();
        std::fs::write(mod_b.join("b.nif"), b"b").unwrap();

        let mut db = ModDatabase::default();
        add_mod(&mut db, &tmp.path().join("mod_a"), "ModA", 0);
        add_mod(&mut db, &tmp.path().join("mod_b"), "ModB", 1);

        let fs = ModUnionFs::build(&game, &db).unwrap();

        let meshes_ino = fs.child_ino_ci(1, "meshes").expect("meshes dir");
        assert!(
            fs.child_ino_ci(meshes_ino, "a.nif").is_some(),
            "a.nif from ModA"
        );
        assert!(
            fs.child_ino_ci(meshes_ino, "b.nif").is_some(),
            "b.nif from ModB"
        );
    }
}
