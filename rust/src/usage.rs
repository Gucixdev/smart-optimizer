//! `where`: what holds the space. One walk, a tree of every folder or file bigger than `--min`, each marked
//! as rebuildable (under a cache directory, the trash, or carrying CACHEDIR.TAG) or not, with how long ago it was
//! last touched. Nothing is listed by name of program; the leaves nobody can vouch for are ranked by idle mass
//! `size * (1 - exp(-age/tau))`, so the biggest and coldest come first. Read only.
use crate::{discover, human, parse_size};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Node {
    pub path: PathBuf,
    pub size: f64,
    pub newest: f64,
    pub tagged: bool,
    pub kids: Vec<Node>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Sizes are on-disk (blocks), hard links once, one filesystem, symlinks not followed. Children smaller than
/// `min` are counted in the parent but not kept.
pub fn walk(path: &Path, st: &fs::Metadata, dev: u64, seen: &mut HashSet<(u64, u64)>, min: f64) -> Node {
    let t = |s: &fs::Metadata| (s.mtime() as f64).max(s.atime() as f64);
    let mut n = Node { path: path.to_path_buf(), size: 0.0, newest: t(st), tagged: false, kids: vec![] };
    if !st.is_dir() {
        if st.nlink() > 1 && !seen.insert((st.dev(), st.ino())) {
            return n;
        }
        n.size = st.blocks() as f64 * 512.0;
        return n;
    }
    n.size = st.blocks() as f64 * 512.0;
    let Ok(rd) = fs::read_dir(path) else { return n };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(cst) = fs::symlink_metadata(&p) else { continue };
        if cst.dev() != dev {
            continue;
        }
        if p.file_name().is_some_and(|f| f == "CACHEDIR.TAG") {
            n.tagged = true;
        }
        let k = walk(&p, &cst, dev, seen, min);
        n.size += k.size;
        n.newest = n.newest.max(k.newest);
        if k.size >= min {
            n.kids.push(k);
        }
    }
    n.kids.sort_by(|a, b| b.size.partial_cmp(&a.size).unwrap());
    n
}

/// Leaves (no kept child) that are not rebuildable, ranked by idle mass.
pub fn idle_ranking(root: &Node, known: &dyn Fn(&Path) -> bool, tau_days: f64, now_s: f64) -> Vec<(f64, f64, f64, PathBuf)> {
    fn go(n: &Node, inherited: bool, known: &dyn Fn(&Path) -> bool, tau: f64, now_s: f64, out: &mut Vec<(f64, f64, f64, PathBuf)>) {
        let rb = inherited || n.tagged || known(&n.path);
        if n.kids.is_empty() {
            if !rb {
                let age = ((now_s - n.newest) / 86400.0).max(0.0);
                out.push((n.size * (1.0 - (-age / tau).exp()), n.size, age, n.path.clone()));
            }
            return;
        }
        for k in &n.kids {
            go(k, rb, known, tau, now_s, out);
        }
    }
    let mut out = Vec::new();
    go(root, false, known, tau_days, now_s, &mut out);
    out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    out
}

fn print(n: &Node, depth: usize, inherited: &str, label: &dyn Fn(&Path) -> &'static str, now_s: f64) {
    let mut kind = inherited;
    let l = label(&n.path);
    if !l.is_empty() {
        kind = l;
    } else if n.tagged && kind.is_empty() {
        kind = "cache";
    }
    let age = ((now_s - n.newest) / 86400.0).max(0.0);
    println!("{:>8}  {:>5.0} d  {:<7} {}{}", human(n.size), age, kind, "  ".repeat(depth), if depth == 0 { n.path.display().to_string() } else { n.path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() });
    for k in &n.kids {
        print(k, depth + 1, kind, label, now_s);
    }
}

pub fn run(args: &[String]) -> i32 {
    let mut dir: Option<String> = None;
    let mut min = "1G".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--min" | "-min" => {
                i += 1;
                match args.get(i) {
                    Some(v) => min = v.clone(),
                    None => {
                        eprintln!("--min needs a size");
                        return 2;
                    }
                }
            }
            a if !a.starts_with('-') && dir.is_none() => dir = Some(a.to_string()),
            a => {
                eprintln!("unexpected argument {a:?}\nusage: smart-optimizer where [DIR] [--min SIZE]");
                return 2;
            }
        }
        i += 1;
    }
    let Some(min) = parse_size(&min) else {
        eprintln!("bad --min");
        return 2;
    };
    let Some(home) = discover::home() else {
        eprintln!("no HOME");
        return 2;
    };
    let root = dir.map(PathBuf::from).unwrap_or_else(|| home.clone());
    let Ok(st) = fs::symlink_metadata(&root) else {
        eprintln!("{}: cannot read", root.display());
        return 1;
    };
    let cache = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".cache"));
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    let trashes = discover::trash_dirs(&home, &mounts, discover::uid());
    let flat = home.join(".var/app");
    let label = move |p: &Path| -> &'static str {
        if p == cache || p == Path::new("/var/cache") || (p.starts_with(&flat) && p.file_name().is_some_and(|f| f == "cache")) {
            "cache"
        } else if trashes.iter().any(|t| p == t) {
            "trash"
        } else {
            ""
        }
    };
    let mut seen = HashSet::new();
    let tree = walk(&root, &st, st.dev(), &mut seen, min);
    let now_s = now();
    print(&tree, 0, "", &label, now_s);
    let kept: f64 = tree.kids.iter().map(|k| k.size).sum();
    println!("\nlisted {} of {} under {}; the rest is in pieces smaller than {}", human(kept), human(tree.size), root.display(), human(min));
    let rank = idle_ranking(&tree, &|p| !label(p).is_empty(), 30.0, now_s);
    if !rank.is_empty() {
        println!("\nnot known to be rebuildable, biggest and coldest first (you decide, nothing here is planned):");
        for (_, size, age, p) in rank.iter().take(10) {
            println!("{:>8}  {:>5.0} d  {}", human(*size), age, p.display());
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_keeps_only_big_nodes_and_ranks_cold_mass() {
        let base = std::env::temp_dir().join(format!("so-where-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("a")).unwrap();
        fs::create_dir_all(base.join("b")).unwrap();
        fs::write(base.join("a/big"), vec![1u8; 1 << 20]).unwrap();
        fs::write(base.join("b/small"), vec![1u8; 4096]).unwrap();
        fs::write(base.join("b/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55").unwrap();
        let st = fs::symlink_metadata(&base).unwrap();
        let tree = walk(&base, &st, st.dev(), &mut HashSet::new(), 512.0 * 1024.0);
        assert_eq!(tree.kids.len(), 1);
        assert!(tree.kids[0].path.ends_with("a") && tree.kids[0].kids[0].path.ends_with("big"));
        let rank = idle_ranking(&tree, &|_| false, 30.0, now() + 100.0 * 86400.0);
        assert_eq!(rank.len(), 1);
        assert!(rank[0].3.ends_with("a/big") && rank[0].2 > 99.0);
        let none = idle_ranking(&tree, &|p| p.ends_with("a"), 30.0, now());
        assert!(none.is_empty(), "everything under a known folder is explained");
        let _ = fs::remove_dir_all(&base);
    }
}
