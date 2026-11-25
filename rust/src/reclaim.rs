//! The planner inputs and the actions on them, kept out of `main` so they can be tested on a fake home:
//! `collect` (every candidate, from standards and the mount table), `apply_items` (the deletion), and the pure
//! logic of `check` (disk free versus a threshold, what could be reclaimed).
use crate::scan::{add_item, scan_root, Item};
use crate::{browsers, discover};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Everything the planner may look at. `sys_cache` is the FHS cache directory (`/var/cache`), `mounts` the text of
/// `/proc/mounts`, `running` the browsers with a live process; all are arguments so a test never touches the system.
#[allow(clippy::too_many_arguments)]
pub fn collect(home: &Path, cache: &Path, sys_cache: &Path, mounts: &str, uid: u32, roots: &[String], running: &HashSet<&str>, tau: f64, bw: f64, split: f64) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    scan_root(&mut items, cache, true, tau, bw, split);
    // flatpak: each app's XDG_CACHE_HOME
    if let Ok(rd) = std::fs::read_dir(home.join(".var/app")) {
        let mut apps: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("cache")).filter(|p| p.is_dir()).collect();
        apps.sort();
        for c in apps {
            scan_root(&mut items, &c, true, tau, bw, split);
        }
    }
    for d in browsers::cache_dirs(home) {
        add_item(&mut items, &d, true, tau, bw);
    }
    // standards, not program names: the FHS cache directory and the freedesktop trash of every mounted disk
    scan_root(&mut items, sys_cache, true, tau, bw, split);
    for t in discover::trash_dirs(home, mounts, uid) {
        discover::add_trash(&mut items, &t, tau, bw);
    }
    for r in roots {
        scan_root(&mut items, Path::new(r), false, tau, bw, split);
    }
    browsers::guard_running(&mut items, running);
    items
}

/// Delete the chosen items (never a protected one); a trashed entry takes its `.trashinfo` along, but only when
/// the entry itself went. Returns the paths removed and the errors, one per failure.
pub fn apply_items(items: &[Item], chosen: &[usize]) -> (Vec<PathBuf>, Vec<String>) {
    let (mut done, mut errs) = (Vec::new(), Vec::new());
    for &i in chosen {
        let it = &items[i];
        if it.protected {
            continue;
        }
        let r = match std::fs::symlink_metadata(&it.path) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&it.path),
            Ok(_) => std::fs::remove_file(&it.path),
            Err(e) => Err(e),
        };
        match r {
            Ok(()) => {
                for x in &it.also {
                    let _ = std::fs::remove_file(x);
                }
                done.push(it.path.clone());
            }
            Err(e) => errs.push(format!("{}: {e}", it.path.display())),
        }
    }
    (done, errs)
}

/// Bytes that can go without asking (cache, trash) and bytes that need root, from the same items the planner uses.
/// A browser that is running protects its cache, which is neither.
pub fn reclaimable(items: &[Item]) -> (f64, f64) {
    let (mut free, mut root) = (0.0, 0.0);
    for it in items {
        if !it.protected {
            free += it.size;
        } else if it.reason.starts_with("needs root") {
            root += it.size;
        }
    }
    (free, root)
}

fn gb(b: f64) -> String {
    format!("{:.1}", b / 1073741824.0)
}

/// The one line `check` prints and whether the disk is at or above the threshold (percent free).
pub fn check_line(total: f64, avail: f64, mount: &str, free: f64, root: f64) -> String {
    let pct = if total > 0.0 { 100.0 * avail / total } else { 0.0 };
    format!("disk: {:.0}% free ({} GB) on {mount}; reclaimable without asking: {} GB (cache, trash, needs-root {} GB)", pct, gb(avail), gb(free), gb(root))
}

/// Exit code of `check`: 0 when free space is at least `below` percent, 3 when under it.
pub fn check_code(total: f64, avail: f64, below: f64) -> i32 {
    if total > 0.0 && 100.0 * avail / total >= below {
        0
    } else {
        3
    }
}

/// `15%` or `15` to a percent in 0..=100.
pub fn parse_percent(s: &str) -> Option<f64> {
    let v: f64 = s.trim().trim_end_matches('%').trim().parse().ok()?;
    (0.0..=100.0).contains(&v).then_some(v)
}

pub fn disk(path: &Path) -> Option<(f64, f64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut v: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut v) } != 0 {
        return None;
    }
    Some((v.f_blocks as f64 * v.f_frsize as f64, v.f_bavail as f64 * v.f_frsize as f64))
}

/// `smart-optimizer check [--below 15%]`: read only; prints one line; exit 0 enough free, 3 below, 2 bad use.
pub fn check(argv: &[String]) -> i32 {
    let mut below = 15.0;
    let mut i = 0;
    while i < argv.len() {
        let (name, inline) = match argv[i].trim_start_matches('-').split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (argv[i].trim_start_matches('-').to_string(), None),
        };
        if !argv[i].starts_with('-') || name != "below" {
            eprintln!("usage: smart-optimizer check [--below 15%]");
            return 2;
        }
        let val = match inline {
            Some(v) => v,
            None => {
                i += 1;
                argv.get(i).cloned().unwrap_or_default()
            }
        };
        let Some(p) = parse_percent(&val) else {
            eprintln!("bad --below {val:?}: want a percent like 15%");
            return 2;
        };
        below = p;
        i += 1;
    }
    let Some(home) = discover::home() else {
        eprintln!("no HOME");
        return 2;
    };
    let cache = crate::cache_root(&home);
    let Some((total, avail)) = disk(Path::new("/")) else {
        eprintln!("cannot read the filesystem of /");
        return 2;
    };
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    // no splitting: only the sums matter here
    let items = collect(&home, &cache, Path::new("/var/cache"), &mounts, discover::uid(), &[], &browsers::running(), 30.0, 5e6, f64::INFINITY);
    let (free, root) = reclaimable(&items);
    println!("{}", check_line(total, avail, "/", free, root));
    check_code(total, avail, below)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn put(p: &Path, n: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![7u8; n]).unwrap();
    }

    /// plan a target, run the deletion, return (items, chosen, removed paths, errors)
    fn run(base: &Path, running: &HashSet<&str>, share: f64) -> (Vec<Item>, Vec<usize>, Vec<PathBuf>, Vec<String>) {
        let (home, cache) = (base.join("home"), base.join("home/.cache"));
        let mounts = format!("/dev/x {} ext4 rw 0 0\n", base.join("disk").display());
        let prot_root = base.join("other");
        let items = collect(&home, &cache, &base.join("no-var-cache"), &mounts, 1000, &[prot_root.display().to_string()], running, 30.0, 5e6, f64::INFINITY);
        let size: Vec<f64> = items.iter().map(|i| i.size).collect();
        let loss: Vec<f64> = items.iter().map(|i| i.loss).collect();
        let prot: Vec<bool> = items.iter().map(|i| i.protected).collect();
        let total: f64 = items.iter().filter(|i| !i.protected).map(|i| i.size).sum();
        let p = crate::plan::plan(&size, &loss, &prot, total * share).expect("feasible");
        let chosen: Vec<usize> = (0..items.len()).filter(|&i| p.chosen[i]).collect();
        let (done, errs) = apply_items(&items, &chosen);
        (items, chosen, done, errs)
    }

    #[test]
    fn apply_removes_cache_and_trash_with_info_and_keeps_the_rest() {
        let base = std::env::temp_dir().join(format!("so-apply-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (home, trash) = (base.join("home"), base.join("home/.local/share/Trash"));
        put(&home.join(".cache/tool/blob"), 40_000);
        put(&home.join(".cache/dir/a"), 20_000);
        put(&home.join(".cache/dir/b"), 20_000);
        put(&trash.join("files/old.bin"), 40_000);
        put(&trash.join("files/folder/x"), 40_000);
        put(&trash.join("info/old.bin.trashinfo"), 20);
        put(&trash.join("info/folder.trashinfo"), 20);
        // an other-disk trash with an entry: removed through its own .Trash-UID
        let disk_trash = base.join("disk/.Trash-1000");
        put(&disk_trash.join("files/big.iso"), 40_000);
        put(&disk_trash.join("info/big.iso.trashinfo"), 20);
        // not a cache: a --root folder without CACHEDIR.TAG is protected; a running browser's cache too
        put(&base.join("other/precious.dat"), 40_000);
        put(&home.join(".cache/mozilla/firefox/p/cache2/entry"), 40_000);
        // profile data next to a Chromium cache must never be touched
        put(&home.join(".config/chromium/Default/Preferences"), 10);
        put(&home.join(".config/chromium/Default/Cookies"), 10);
        put(&home.join(".config/chromium/Default/Cache/f"), 40_000);

        // every item is a sixth of the total, so a 90% target needs all six (the grid cannot reach 99%, it rounds down)
        let (items, chosen, done, errs) = run(&base, &HashSet::from(["firefox"]), 0.9);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(done.len(), chosen.len());
        assert!(chosen.iter().all(|&i| !items[i].protected));
        for gone in [".cache/tool", ".cache/dir/a", ".cache/dir/b", ".local/share/Trash/files/old.bin", ".local/share/Trash/files/folder", ".local/share/Trash/info/old.bin.trashinfo", ".local/share/Trash/info/folder.trashinfo", ".config/chromium/Default/Cache"] {
            assert!(!home.join(gone).exists(), "{gone} should be gone");
        }
        assert!(!base.join("disk/.Trash-1000/files/big.iso").exists());
        assert!(!disk_trash.join("info/big.iso.trashinfo").exists(), "the .trashinfo goes with its file");
        for stays in ["other/precious.dat", "home/.cache/mozilla/firefox/p/cache2/entry", "home/.config/chromium/Default/Preferences", "home/.config/chromium/Default/Cookies"] {
            assert!(base.join(stays).exists(), "{stays} must stay");
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_with_a_small_target_removes_only_what_was_chosen() {
        let base = std::env::temp_dir().join(format!("so-apply2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let trash = base.join("home/.local/share/Trash");
        for n in ["a", "b", "c", "d"] {
            put(&base.join("home/.cache").join(n).join("f"), 40_000);
            put(&trash.join("files").join(n), 40_000);
            put(&trash.join("info").join(format!("{n}.trashinfo")), 20);
        }
        let (items, chosen, done, errs) = run(&base, &HashSet::new(), 0.4);
        assert!(errs.is_empty() && !chosen.is_empty() && chosen.len() < items.len());
        for (i, it) in items.iter().enumerate() {
            assert_eq!(it.path.exists(), !chosen.contains(&i), "{}", it.path.display());
            for x in &it.also {
                assert_eq!(x.exists(), !chosen.contains(&i), "{}", x.display());
            }
        }
        assert_eq!(done.len(), chosen.len());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn protected_item_is_never_deleted_even_when_listed() {
        let base = std::env::temp_dir().join(format!("so-apply3-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        put(&base.join("keep/f"), 100);
        let it = Item { path: base.join("keep"), size: 1.0, loss: 1.0, protected: true, reason: "x".into(), also: vec![] };
        let (done, errs) = apply_items(&[it], &[0]);
        assert!(done.is_empty() && errs.is_empty() && base.join("keep/f").exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn check_threshold_line_and_sums() {
        let g = 1073741824.0;
        assert_eq!(check_code(100.0 * g, 15.0 * g, 15.0), 0);
        assert_eq!(check_code(100.0 * g, 14.9 * g, 15.0), 3);
        assert_eq!(check_code(0.0, 0.0, 15.0), 3);
        assert_eq!(check_line(118.0 * g, 17.0 * g, "/", 2.04 * g, 0.5 * g), "disk: 14% free (17.0 GB) on /; reclaimable without asking: 2.0 GB (cache, trash, needs-root 0.5 GB)");
        assert_eq!((parse_percent("15%"), parse_percent("7.5"), parse_percent("x"), parse_percent("101%")), (Some(15.0), Some(7.5), None, None));
        let mk = |size, protected, reason: &str| Item { path: PathBuf::from("/x"), size, loss: 1.0, protected, reason: reason.into(), also: vec![] };
        let items = [mk(10.0, false, ""), mk(5.0, true, "needs root (run again with sudo)"), mk(7.0, true, "firefox is running"), mk(3.0, false, "")];
        assert_eq!(reclaimable(&items), (13.0, 5.0));
        assert_eq!(check(&["--below".into(), "x".into()]), 2);
        assert_eq!(check(&["bogus".into()]), 2);
    }
}
