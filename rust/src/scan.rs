//! Scanning: the on-disk size and newest time of a tree, the loss of an item, the cache-root children.
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One thing that could be freed: a top-level entry of a cache root, or a browser cache directory.
#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub size: f64,
    pub loss: f64,
    pub protected: bool,
    pub reason: String, // why it is protected, when it is
    pub also: Vec<PathBuf>, // removed together with the item (the .trashinfo of a trashed file)
}

pub const NOT_CACHE: &str = "not a cache (no cache root, no CACHEDIR.TAG)";

/// True when the user running us may remove `path`: write and search permission on its parent directory.
pub fn removable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Some(parent) = path.parent() else { return false };
    let Ok(c) = std::ffi::CString::new(parent.as_os_str().as_bytes()) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

/// An item the user cannot remove is protected, with the reason, so the plan says what running as root would add.
pub fn needs_root(it: &mut Item) {
    if !it.protected && !removable(&it.path) {
        it.protected = true;
        it.reason = "needs root (run again with sudo)".into();
    }
}

pub struct Measure {
    pub size: f64,
    pub newest: f64, // unix seconds
    pub tagged: bool,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// On-disk size (blocks*512, hard links counted once), newest mtime/atime, whether the tree carries a
/// CACHEDIR.TAG. Stays on one filesystem, never follows symlinks.
pub fn measure(root: &Path) -> Measure {
    let mut m = Measure { size: 0.0, newest: 0.0, tagged: false };
    let Ok(st) = fs::symlink_metadata(root) else { return m };
    let mut seen = HashSet::new();
    walk(root, &st, st.dev(), &mut seen, &mut m);
    m
}

fn walk(path: &Path, st: &fs::Metadata, dev: u64, seen: &mut HashSet<(u64, u64)>, m: &mut Measure) {
    let is_dir = st.is_dir();
    if !is_dir && st.nlink() > 1 && !seen.insert((st.dev(), st.ino())) {
        return;
    }
    m.size += st.blocks() as f64 * 512.0;
    for t in [st.mtime() as f64 + st.mtime_nsec() as f64 / 1e9, st.atime() as f64 + st.atime_nsec() as f64 / 1e9] {
        if t > m.newest {
            m.newest = t;
        }
    }
    if path.file_name().is_some_and(|n| n == "CACHEDIR.TAG") {
        m.tagged = true;
    }
    if !is_dir {
        return;
    }
    let Ok(rd) = fs::read_dir(path) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(cst) = fs::symlink_metadata(&p) else { continue };
        if cst.dev() != dev {
            continue; // another filesystem: neither the entry nor what is below it
        }
        walk(&p, &cst, dev, seen, m);
    }
}

/// loss = exp(-age/tau) * (1 + size/bw): the chance it is needed again times the time to get it back.
pub fn loss(size: f64, newest: f64, tau_days: f64, bw: f64) -> f64 {
    let age = ((now() - newest) / 86400.0).max(0.0);
    (-age / tau_days).exp() * (1.0 + size / bw)
}

/// Measure one path and append it. rebuildable = lives in a cache root or carries a CACHEDIR.TAG.
pub fn add_item(items: &mut Vec<Item>, path: &Path, rebuildable: bool, tau: f64, bw: f64) {
    let m = measure(path);
    if m.size <= 0.0 {
        return;
    }
    let protected = !(rebuildable || m.tagged);
    let mut it = Item {
        path: path.to_path_buf(),
        size: m.size,
        loss: loss(m.size, m.newest, tau, bw),
        protected,
        reason: if protected { NOT_CACHE.into() } else { String::new() },
        also: vec![],
    };
    needs_root(&mut it);
    items.push(it);
}

/// Like `add_item`, but an item bigger than `split_above` is replaced by its children (down to `depth` levels),
/// so the planner can free part of a huge folder instead of all of it or nothing. A tagged folder makes its
/// children rebuildable too.
pub fn add_split(items: &mut Vec<Item>, path: &Path, rebuildable: bool, tau: f64, bw: f64, split_above: f64, depth: u32) {
    if depth > 0 && fs::symlink_metadata(path).map(|s| s.is_dir()).unwrap_or(false) {
        let m = measure(path);
        if m.size > split_above {
            if let Ok(rd) = fs::read_dir(path) {
                let mut kids: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
                if !kids.is_empty() {
                    kids.sort();
                    for k in kids {
                        add_split(items, &k, rebuildable || m.tagged, tau, bw, split_above, depth - 1);
                    }
                    return;
                }
            }
        }
    }
    add_item(items, path, rebuildable, tau, bw);
}

/// Every direct child of root, split when bigger than `split_above`.
pub fn scan_root(items: &mut Vec<Item>, root: &Path, rebuildable: bool, tau: f64, bw: f64, split_above: f64) {
    let Ok(rd) = fs::read_dir(root) else { return };
    let mut kids: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    kids.sort();
    for k in kids {
        add_split(items, &k, rebuildable, tau, bw, split_above, 3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_folder_is_cut_into_children_small_one_is_not() {
        let base = std::env::temp_dir().join(format!("so-split-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("pkg")).unwrap();
        for n in ["a", "b", "c"] {
            fs::write(base.join("pkg").join(n), vec![1u8; 64 * 1024]).unwrap();
        }
        fs::create_dir_all(base.join("tiny")).unwrap();
        fs::write(base.join("tiny/x"), "x").unwrap();
        let mut items = Vec::new();
        scan_root(&mut items, &base, true, 30.0, 5e6, 100.0 * 1024.0);
        let names: Vec<String> = items.iter().map(|i| i.path.strip_prefix(&base).unwrap().display().to_string()).collect();
        assert_eq!(names, vec!["pkg/a", "pkg/b", "pkg/c", "tiny"]);
        assert!(items.iter().all(|i| !i.protected));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn unwritable_parent_means_needs_root() {
        let mut it = Item { path: PathBuf::from("/usr/bin/true"), size: 1.0, loss: 1.0, protected: false, reason: String::new(), also: vec![] };
        needs_root(&mut it);
        assert_eq!(it.protected, unsafe { libc::geteuid() } != 0);
    }
}
