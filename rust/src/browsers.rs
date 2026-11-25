//! Browsers: only HTTP, code and shader caches are ever planned. Never profile data: cookies, logins, bookmarks,
//! history, extensions, local storage, IndexedDB, sessions. Chromium-family profiles keep their caches under
//! ~/.config/<browser>/<profile>/ next to that data, so the cache directory names are listed, nothing else.
use crate::scan::Item;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const CHROMIUM_CACHE_DIRS: [&str; 7] = ["Cache", "Code Cache", "GPUCache", "DawnCache", "GrShaderCache", "ShaderCache", "GraphiteDawnCache"];

pub struct Browser {
    pub name: &'static str,
    pub procs: &'static [&'static str], // /proc/*/comm names that mean it is running
    pub marks: &'static [&'static str], // a path with a component equal to one of these (or starting with it and a dot) belongs to this browser
}

pub const BROWSERS: &[Browser] = &[
    Browser { name: "firefox", procs: &["firefox", "firefox-bin", "Isolated Web Co"], marks: &["mozilla", "firefox", "org.mozilla.firefox", "librewolf", "waterfox"] },
    Browser { name: "chrome", procs: &["chrome", "google-chrome"], marks: &["google-chrome", "com.google.Chrome"] },
    Browser { name: "chromium", procs: &["chromium", "chromium-browser"], marks: &["chromium", "org.chromium"] },
    Browser { name: "brave", procs: &["brave", "brave-browser"], marks: &["BraveSoftware", "brave", "com.brave.Browser"] },
    Browser { name: "vivaldi", procs: &["vivaldi-bin", "vivaldi"], marks: &["vivaldi", "com.vivaldi.Vivaldi"] },
    Browser { name: "edge", procs: &["msedge", "microsoft-edge"], marks: &["microsoft-edge", "com.microsoft.Edge"] },
    Browser { name: "opera", procs: &["opera"], marks: &["opera", "com.opera.Opera"] },
];

fn dirs(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(p).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// <cfg>/<a>/<b> and <cfg>/<a>/<b>/<c> directories that hold a Chromium "Preferences" file.
fn profiles(cfg: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in dirs(cfg) {
        for b in dirs(&a) {
            if b.join("Preferences").is_file() {
                out.push(b.clone());
            }
            for c in dirs(&b) {
                if c.join("Preferences").is_file() {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// Chromium-family cache directories that live outside the XDG cache root (also inside Flatpak configs).
pub fn cache_dirs(home: &Path) -> Vec<PathBuf> {
    let mut cfgs = vec![home.join(".config")];
    for app in dirs(&home.join(".var/app")) {
        cfgs.push(app.join("config"));
    }
    let mut out = Vec::new();
    for cfg in cfgs {
        for prof in profiles(&cfg) {
            for d in CHROMIUM_CACHE_DIRS {
                let p = prof.join(d);
                if p.is_dir() {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Browsers with a live process (read from /proc/*/comm).
pub fn running() -> HashSet<&'static str> {
    let mut run = HashSet::new();
    let Ok(rd) = fs::read_dir("/proc") else { return run };
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(name) = fs::read_to_string(e.path().join("comm")) else { continue };
        let name = name.trim();
        for br in BROWSERS {
            if br.procs.contains(&name) {
                run.insert(br.name);
            }
        }
    }
    run
}

/// True when a whole path component names the browser: `mozilla`, `.mozilla`, `org.chromium.Chromium` yes,
/// `ca-certificates-mozilla-3.1.pkg.tar.zst` no (a substring match protected a pacman package here).
fn has_mark(path: &Path, marks: &[&str]) -> bool {
    path.components().any(|c| {
        let c = c.as_os_str().to_string_lossy();
        let c = c.trim_start_matches('.');
        marks.iter().any(|m| c == *m || c.strip_prefix(m).is_some_and(|r| r.starts_with('.')))
    })
}

/// Items of a browser that is running right now are protected: its cache is open and locked.
pub fn guard_running(items: &mut [Item], running: &HashSet<&str>) {
    for it in items.iter_mut() {
        for br in BROWSERS.iter().filter(|b| running.contains(b.name)) {
            if has_mark(&it.path, br.marks) {
                it.protected = true;
                it.reason = format!("{} is running", br.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(p: &Path, file: bool) {
        if file {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        } else {
            fs::create_dir_all(p).unwrap();
        }
    }

    #[test]
    fn chromium_caches_only_never_profile_data() {
        let home = std::env::temp_dir().join(format!("so-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let prof = home.join(".config/google-chrome/Default");
        mk(&prof.join("Preferences"), true);
        for d in ["Cache", "Code Cache", "GPUCache", "Local Storage", "IndexedDB", "Extensions"] {
            mk(&prof.join(d), false);
        }
        mk(&prof.join("Cookies"), true);
        mk(&prof.join("Login Data"), true);
        let flat = home.join(".var/app/org.chromium.Chromium/config/chromium/Default");
        mk(&flat.join("Preferences"), true);
        mk(&flat.join("GPUCache"), false);

        let got: HashSet<PathBuf> = cache_dirs(&home).into_iter().collect();
        for want in ["Cache", "Code Cache", "GPUCache"] {
            assert!(got.contains(&prof.join(want)), "missing {want}");
        }
        assert!(got.contains(&flat.join("GPUCache")), "flatpak Chromium cache not found");
        for never in ["Local Storage", "IndexedDB", "Extensions", "Cookies", "Login Data"] {
            assert!(!got.contains(&prof.join(never)), "profile data {never} must never be a candidate");
        }
        let _ = fs::remove_dir_all(&home);
    }

    fn item(p: &str) -> Item {
        Item { path: PathBuf::from(p), size: 1.0, loss: 1.0, protected: false, reason: String::new(), also: vec![] }
    }

    #[test]
    fn running_browser_cache_is_protected() {
        let mut items = vec![
            item("/h/.cache/mozilla/firefox/x/cache2"),
            item("/h/.cache/google-chrome/Default/Cache"),
            item("/h/.cache/zig"),
            item("/var/cache/pacman/pkg/ca-certificates-mozilla-3.132-1.1-x86_64.pkg.tar.zst"),
            item("/h/.var/app/org.mozilla.firefox/cache/x"),
            item("/h/.mozilla/firefox/p/cache"),
        ];
        guard_running(&mut items, &HashSet::from(["firefox"]));
        let got: Vec<bool> = items.iter().map(|i| i.protected).collect();
        assert_eq!(got, vec![true, false, false, false, true, true]);
        assert_eq!(items[0].reason, "firefox is running");
    }
}
