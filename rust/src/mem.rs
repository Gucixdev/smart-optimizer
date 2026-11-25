//! `mem` only reads: /proc/meminfo, /proc/swaps, /sys/block/zram*, nvidia-smi when present.
//! It says what could be switched off and why, and prints the command. Nothing is executed.
use crate::human;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::process::Command;

pub fn report(w: &mut dyn Write) {
    let mi = read_kv("/proc/meminfo");
    let get = |k: &str| mi.get(k).copied().unwrap_or(0.0);
    let (total, avail) = (get("MemTotal"), get("MemAvailable"));
    let _ = writeln!(w, "RAM   {} total, {} available, {} page cache", human(total), human(avail), human(get("Cached")));

    struct Swap {
        name: String,
        size: f64,
        used: f64,
        prio: i64,
    }
    let mut swaps = Vec::new();
    if let Ok(b) = fs::read_to_string("/proc/swaps") {
        for ln in b.lines().skip(1) {
            let f: Vec<&str> = ln.split_whitespace().collect();
            if f.len() < 5 {
                continue;
            }
            swaps.push(Swap {
                name: f[0].to_string(),
                size: f[2].parse::<f64>().unwrap_or(0.0) * 1024.0,
                used: f[3].parse::<f64>().unwrap_or(0.0) * 1024.0,
                prio: f[4].parse().unwrap_or(0),
            });
        }
    }
    let swap_total: f64 = swaps.iter().map(|s| s.size).sum();
    let swap_used: f64 = swaps.iter().map(|s| s.used).sum();
    let _ = writeln!(w, "swap  {} total, {} used, swappiness {}", human(swap_total), human(swap_used), read_trim("/proc/sys/vm/swappiness"));
    let margin = total * 0.10;
    for s in &swaps {
        let verdict = if s.used == 0.0 {
            "idle: costs no RAM, only disk".to_string()
        } else if s.used < avail - margin {
            format!("its {} would fit back into RAM: swapoff {}", human(s.used), s.name)
        } else {
            "in use and RAM cannot take it back".to_string()
        };
        let _ = writeln!(w, "      prio {:<4} {:>8} used of {:<8} {}  -> {}", s.prio, human(s.used), human(s.size), s.name, verdict);
    }
    if total > 0.0 && swap_total > 2.0 * total {
        let _ = writeln!(w, "note  swap is {:.1}x RAM; the idle part is only disk space", swap_total / total);
    }
    let mut zr: Vec<_> = fs::read_dir("/sys/block").map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    zr.sort();
    for z in zr.iter().filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("zram"))) {
        let orig = read_num(&z.join("orig_data_size").to_string_lossy());
        let mem = read_num(&z.join("mem_used_total").to_string_lossy());
        if orig > 0.0 {
            let _ = writeln!(w, "zram  {}: {} of data held in {} of RAM ({:.1}x)", z.file_name().unwrap().to_string_lossy(), human(orig), human(mem), orig / mem.max(1.0));
        }
    }
    match Command::new("nvidia-smi").args(["--query-gpu=memory.used,memory.total", "--format=csv,noheader,nounits"]).output() {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            let f: Vec<&str> = out.trim().split(',').collect();
            if f.len() == 2 {
                let u: f64 = f[0].trim().parse().unwrap_or(0.0);
                let t: f64 = f[1].trim().parse().unwrap_or(0.0);
                let _ = writeln!(w, "VRAM  {} used of {}", human(u * 1048576.0), human(t * 1048576.0));
            }
            if let Ok(a) = Command::new("nvidia-smi").args(["--query-compute-apps=pid,used_memory,process_name", "--format=csv,noheader"]).output() {
                for ln in String::from_utf8_lossy(&a.stdout).trim().lines().filter(|l| !l.is_empty()) {
                    let _ = writeln!(w, "      compute: {ln}");
                }
            }
        }
        _ => {
            let _ = writeln!(w, "VRAM  not checked (no nvidia-smi)");
        }
    }
}

fn read_kv(p: &str) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for ln in fs::read_to_string(p).unwrap_or_default().lines() {
        let Some((k, v)) = ln.split_once(':') else { continue };
        let f: Vec<&str> = v.split_whitespace().collect();
        let Some(first) = f.first() else { continue };
        let mut n: f64 = first.parse().unwrap_or(0.0);
        if f.get(1) == Some(&"kB") {
            n *= 1024.0;
        }
        m.insert(k.trim().to_string(), n);
    }
    m
}

fn read_trim(p: &str) -> String {
    fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_else(|_| "?".into())
}

fn read_num(p: &str) -> f64 {
    read_trim(p).parse().unwrap_or(0.0)
}
