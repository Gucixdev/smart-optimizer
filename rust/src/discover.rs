//! Where things can be freed, found from standards and from the mount table, never from a list of programs:
//! the XDG trash (home and one per mounted disk), the FHS cache directory, and users of `SUDO_USER`.
use crate::scan::{self, Item};
use std::fs;
use std::path::{Path, PathBuf};

/// The user the tool works for: the one who called sudo, else the current one.
pub fn uid() -> u32 {
    std::env::var("SUDO_UID").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| unsafe { libc::getuid() })
}

/// Home of that user. Under sudo `HOME` may point at /root, so the passwd entry of `SUDO_USER` wins.
pub fn home() -> Option<PathBuf> {
    if let Some(name) = std::env::var_os("SUDO_USER") {
        use std::os::unix::ffi::OsStrExt;
        if let Ok(c) = std::ffi::CString::new(name.as_bytes()) {
            let pw = unsafe { libc::getpwnam(c.as_ptr()) };
            if !pw.is_null() {
                let dir = unsafe { std::ffi::CStr::from_ptr((*pw).pw_dir) };
                return Some(PathBuf::from(dir.to_string_lossy().into_owned()));
            }
        }
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Mount points of real (device-backed) filesystems, read from /proc/mounts text.
pub fn mount_points(mounts: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for line in mounts.lines() {
        let mut f = line.split_whitespace();
        let (Some(src), Some(mp)) = (f.next(), f.next()) else { continue };
        if !src.starts_with('/') {
            continue;
        }
        let p = PathBuf::from(mp.replace("\\040", " ").replace("\\011", "\t"));
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Trash directories of the freedesktop trash specification: the home trash and, on every mounted disk,
/// `.Trash-UID` or `.Trash/UID`. A directory counts when it holds `files/` and `info/`.
pub fn trash_dirs(home: &Path, mounts: &str, uid: u32) -> Vec<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let mut cands = vec![data.join("Trash")];
    for mp in mount_points(mounts) {
        cands.push(mp.join(format!(".Trash-{uid}")));
        cands.push(mp.join(".Trash").join(uid.to_string()));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for c in cands {
        if c.join("files").is_dir() && c.join("info").is_dir() && !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// One item per trashed entry; its age is the time it was trashed (the `.trashinfo` file), not the age of the
/// file inside. Removing the item removes the info file with it.
pub fn add_trash(items: &mut Vec<Item>, trash: &Path, tau: f64, bw: f64) {
    let Ok(rd) = fs::read_dir(trash.join("files")) else { return };
    let mut kids: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    kids.sort();
    for k in kids {
        let name = k.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let info = trash.join("info").join(format!("{name}.trashinfo"));
        let m = scan::measure(&k);
        if m.size <= 0.0 {
            continue;
        }
        let newest = fs::metadata(&info).ok().and_then(|i| i.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(m.newest);
        let mut it = Item { path: k.clone(), size: m.size, loss: scan::loss(m.size, newest, tau, bw), protected: false, reason: String::new(), also: vec![info] };
        scan::needs_root(&mut it);
        items.push(it);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounts_keep_devices_only() {
        let t = "/dev/sda1 / ext4 rw 0 0\ntmpfs /tmp tmpfs rw 0 0\n/dev/sdb1 /data/my\\040disk ext4 rw 0 0\nproc /proc proc rw 0 0\n/dev/sda1 / ext4 rw 0 0\n";
        assert_eq!(mount_points(t), vec![PathBuf::from("/"), PathBuf::from("/data/my disk")]);
    }

    #[test]
    fn trash_found_on_every_mount_and_needs_both_folders() {
        let base = std::env::temp_dir().join(format!("so-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (home, disk) = (base.join("home"), base.join("disk"));
        for d in [home.join(".local/share/Trash/files"), home.join(".local/share/Trash/info"), disk.join(".Trash-1234/files"), disk.join(".Trash-1234/info"), disk.join(".Trash-99/files")] {
            fs::create_dir_all(d).unwrap();
        }
        let mounts = format!("/dev/x {} ext4 rw 0 0\n", disk.display());
        let got = trash_dirs(&home, &mounts, 1234);
        assert_eq!(got, vec![home.join(".local/share/Trash"), disk.join(".Trash-1234")]);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn trashed_entry_ages_by_its_info_file_and_removes_it_too() {
        let base = std::env::temp_dir().join(format!("so-trash2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let t = base.join("Trash");
        fs::create_dir_all(t.join("files")).unwrap();
        fs::create_dir_all(t.join("info")).unwrap();
        fs::write(t.join("files/a.bin"), vec![1u8; 8192]).unwrap();
        fs::write(t.join("info/a.bin.trashinfo"), "[Trash Info]\n").unwrap();
        let mut items = Vec::new();
        add_trash(&mut items, &t, 30.0, 5e6);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].also, vec![t.join("info/a.bin.trashinfo")]);
        assert!(items[0].size >= 8192.0 && !items[0].protected);
        let _ = fs::remove_dir_all(&base);
    }
}
