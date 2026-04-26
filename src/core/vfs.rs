use fuser::{
    BackgroundSession, FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData,
    ReplyDirectory, ReplyEntry, Request,
};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::core::games::Game;
use crate::core::mods::ModDatabase;

const TTL: Duration = Duration::from_secs(1);

// ── Node types ────────────────────────────────────────────────────────────────

enum VfsNodeKind {
    Dir {
        parent: u64,
        children: HashMap<OsString, u64>,
    },
    File {
        /// Path that can be opened without going through the FUSE mount.
        /// For real game files this is /proc/self/fd/<N>/relative; for mod
        /// files it is the absolute path under the mod's source directory.
        read_path: PathBuf,
    },
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
}

impl ModUnionFs {
    fn build(game: &Game, db: &ModDatabase) -> Result<Self, String> {
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
        };

        // Root inode = 1 (required by FUSE spec)
        fs.nodes.insert(1, VfsNode {
            attr: dir_attr(1, uid, gid, now),
            kind: VfsNodeKind::Dir { parent: 1, children: HashMap::new() },
        });

        let mut next_ino = 2u64;

        // Layer 0: real game Data/ (lowest priority — every mod overrides it)
        if !game_proc_path.as_os_str().is_empty() {
            fs.overlay(&game_proc_path, 1, &mut next_ino, uid, gid, now)?;
        }

        // Layers 1‥N: enabled mods, ascending priority (highest number wins)
        let mut mods: Vec<_> = db.mods.iter().filter(|m| m.enabled).collect();
        mods.sort_by_key(|m| m.priority);
        for m in mods {
            let mod_data = m.source_path.join("Data");
            let root = if mod_data.is_dir() { mod_data } else { m.source_path.clone() };
            if root.is_dir() {
                fs.overlay(&root, 1, &mut next_ino, uid, gid, now)?;
            }
        }

        log::debug!(
            "VFS built: {} inodes for {} enabled mods",
            fs.nodes.len(),
            db.mods.iter().filter(|m| m.enabled).count()
        );
        Ok(fs)
    }

    /// Merge `src_dir` into the VFS directory at `parent_ino`.
    /// Higher-priority files silently replace lower-priority ones.
    fn overlay(
        &mut self,
        src_dir: &Path,
        parent_ino: u64,
        next_ino: &mut u64,
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
            let Ok(meta) = std::fs::metadata(&src) else { continue };
            let name = entry.file_name();
            let name_lc = name.to_string_lossy().to_lowercase();
            let existing = self.child_ino_ci(parent_ino, &name_lc);

            if meta.is_dir() {
                let child_ino = match existing {
                    Some(ino) => ino,
                    None => {
                        let ino = *next_ino;
                        *next_ino += 1;
                        self.nodes.insert(ino, VfsNode {
                            attr: dir_attr(ino, uid, gid, now),
                            kind: VfsNodeKind::Dir { parent: parent_ino, children: HashMap::new() },
                        });
                        self.insert_child(parent_ino, name.clone(), ino);
                        ino
                    }
                };
                self.overlay(&src, child_ino, next_ino, uid, gid, now)?;
            } else if meta.is_file() {
                let size = meta.len();
                let mtime = meta.modified().unwrap_or(now);
                match existing {
                    Some(ino) => {
                        if let Some(node) = self.nodes.get_mut(&ino) {
                            node.attr.size = size;
                            node.attr.mtime = mtime;
                            node.attr.blocks = blocks(size);
                            node.kind = VfsNodeKind::File { read_path: src };
                        }
                    }
                    None => {
                        let ino = *next_ino;
                        *next_ino += 1;
                        self.nodes.insert(ino, VfsNode {
                            attr: file_attr(ino, size, mtime, uid, gid, now),
                            kind: VfsNodeKind::File { read_path: src },
                        });
                        self.insert_child(parent_ino, name.clone(), ino);
                    }
                }
            }
        }
        Ok(())
    }

    fn child_ino_ci(&self, parent: u64, name_lc: &str) -> Option<u64> {
        let node = self.nodes.get(&parent)?;
        if let VfsNodeKind::Dir { children, .. } = &node.kind {
            children.iter()
                .find(|(k, _)| k.to_string_lossy().to_lowercase() == name_lc)
                .map(|(_, &ino)| ino)
        } else {
            None
        }
    }

    fn insert_child(&mut self, parent: u64, name: OsString, child: u64) {
        if let Some(VfsNode { kind: VfsNodeKind::Dir { children, .. }, .. }) =
            self.nodes.get_mut(&parent)
        {
            children.insert(name, child);
        }
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
            Some(node) => reply.entry(&TTL, &node.attr, 0),
            None => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        match self.nodes.get(&ino) {
            Some(node) => reply.attr(&TTL, &node.attr),
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
        let VfsNodeKind::File { read_path } = &node.kind else {
            reply.error(libc::EISDIR);
            return;
        };
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
            let kind = match self.nodes.get(&child_ino) {
                Some(VfsNode { kind: VfsNodeKind::Dir { .. }, .. }) => FileType::Directory,
                _ => FileType::RegularFile,
            };
            entries.push((child_ino, kind, name.clone()));
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

fn file_attr(ino: u64, size: u64, mtime: SystemTime, uid: u32, gid: u32, now: SystemTime) -> FileAttr {
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
        MountOption::FSName("linkmm".to_string()),
        MountOption::Subtype("modvfs".to_string()),
        MountOption::AutoUnmount,
        MountOption::NoDev,
        MountOption::NoSuid,
        MountOption::NoExec,
    ];

    let session = fuser::spawn_mount2(fs, &mountpoint, options)
        .map_err(|e| format!("Failed to mount mod VFS at {}: {e}", mountpoint.display()))?;

    log::info!("Mounted mod VFS at {}", mountpoint.display());
    Ok(MountHandle { _session: session, mountpoint })
}

impl Drop for MountHandle {
    fn drop(&mut self) {
        log::info!("Unmounting mod VFS at {}", self.mountpoint.display());
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
        let tex_ino = fs.child_ino_ci(textures_ino, "tex.dds").expect("tex.dds in VFS");
        let node = fs.nodes.get(&tex_ino).unwrap();
        if let VfsNodeKind::File { read_path } = &node.kind {
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
        assert!(fs.child_ino_ci(meshes_ino, "a.nif").is_some(), "a.nif from ModA");
        assert!(fs.child_ino_ci(meshes_ino, "b.nif").is_some(), "b.nif from ModB");
    }
}
