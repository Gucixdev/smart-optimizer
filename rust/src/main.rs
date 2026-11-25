//! smart-optimizer: free disk space at the least expected loss, and report what RAM, swap and VRAM are doing.
//! One binary, no config, no daemon, nothing is changed without --apply.
//!
//!   smart-optimizer [--free SIZE] [--tau DAYS] [--bw BYTES/S] [--root DIR]... [--apply]
//!   smart-optimizer mem
//!   smart-optimizer check [--below 15%]   (read only, one line, exit 0 enough free, 3 below)
//!   smart-optimizer where [DIR] [--min SIZE]   (what holds the space: a tree of the big folders, nothing hardcoded)
//!   smart-optimizer --plan SIZE        (stdin: "size loss protected" per line; prints the loss of the plan)
mod browsers;
mod discover;
mod mem;
mod plan;
mod reclaim;
mod scan;
mod usage;

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::exit;

const USAGE: &str = "usage: smart-optimizer [--free SIZE] [--tau DAYS] [--bw BYTES/S] [--root DIR]... [--apply]\n       smart-optimizer mem\n       smart-optimizer check [--below 15%]\n       smart-optimizer where [DIR] [--min SIZE]\n       smart-optimizer --plan SIZE   (stdin: \"size loss protected\")";

pub fn parse_size(s: &str) -> Option<f64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last()? {
        'k' | 'K' => (&s[..s.len() - 1], 1024.0),
        'm' | 'M' => (&s[..s.len() - 1], 1048576.0),
        'g' | 'G' => (&s[..s.len() - 1], 1073741824.0),
        't' | 'T' => (&s[..s.len() - 1], 1099511627776.0),
        _ => (s, 1.0),
    };
    num.parse::<f64>().ok().map(|v| v * mult)
}

pub fn human(mut b: f64) -> String {
    let u = ["B", "K", "M", "G", "T"];
    let mut i = 0;
    while b >= 1024.0 && i < u.len() - 1 {
        b /= 1024.0;
        i += 1;
    }
    format!("{:.1}{}", b, u[i])
}

/// C/Go `%.{prec}g`: `prec` significant digits, trailing zeros dropped, exponent form when it is < -4 or >= prec.
pub fn fmt_g(v: f64, prec: usize) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if !v.is_finite() {
        return format!("{v}");
    }
    let e = format!("{:.*e}", prec.saturating_sub(1), v); // d.ddde[-]x
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mant.starts_with('-');
    let mut digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    let nd = digits.len() as i32;
    let dp = exp + 1;
    let mut eprec = prec as i32;
    if eprec > nd && nd >= dp {
        eprec = nd;
    }
    let sign = if neg { "-" } else { "" };
    if exp < -4 || exp >= eprec {
        let frac = if nd > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        return format!("{sign}{}{frac}e{}{:02}", &digits[..1], if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    if dp <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-dp) as usize))
    } else if nd <= dp {
        format!("{sign}{digits}{}", "0".repeat((dp - nd) as usize))
    } else {
        format!("{sign}{}.{}", &digits[..dp as usize], &digits[dp as usize..])
    }
}

struct Args {
    free: Option<String>,
    plan: Option<String>,
    tau: f64,
    bw: String,
    apply: bool,
    roots: Vec<String>,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args { free: None, plan: None, tau: 30.0, bw: "5M".into(), apply: false, roots: vec![] };
    let mut i = 0;
    while i < argv.len() {
        let raw = argv[i].trim_start_matches('-');
        if !argv[i].starts_with('-') || raw.is_empty() {
            return Err(format!("unexpected argument {:?}", argv[i]));
        }
        let (name, inline) = match raw.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (raw, None),
        };
        if name == "apply" {
            a.apply = inline.is_none_or(|v| v != "false");
            i += 1;
            continue;
        }
        if matches!(name, "h" | "help") {
            println!("{USAGE}");
            exit(0);
        }
        let val = match inline {
            Some(v) => v,
            None => {
                i += 1;
                argv.get(i).cloned().ok_or(format!("flag needs an argument: --{name}"))?
            }
        };
        match name {
            "free" => a.free = Some(val),
            "plan" => a.plan = Some(val),
            "bw" => a.bw = val,
            "root" => a.roots.push(val),
            "tau" => a.tau = val.parse().map_err(|_| "bad --tau".to_string())?,
            _ => return Err(format!("flag provided but not defined: --{name}")),
        }
        i += 1;
    }
    Ok(a)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("mem") {
        mem::report(&mut std::io::stdout());
        return;
    }
    if argv.first().map(String::as_str) == Some("check") {
        exit(reclaim::check(&argv[1..]));
    }
    if argv.first().map(String::as_str) == Some("where") {
        exit(usage::run(&argv[1..]));
    }
    let a = parse_args(&argv).unwrap_or_else(|e| {
        eprintln!("{e}\n{USAGE}");
        exit(2)
    });
    let bw = match parse_size(&a.bw) {
        Some(v) if v > 0.0 => v,
        _ => {
            eprintln!("bad --bw");
            exit(2)
        }
    };
    if let Some(p) = &a.plan {
        let Some(target) = parse_size(p) else {
            eprintln!("bad --plan");
            exit(2)
        };
        exit(plan_stdin(target));
    }

    let Some(home) = discover::home() else {
        eprintln!("no HOME");
        exit(2)
    };
    let cache = cache_root(&home);
    let target = match &a.free {
        Some(f) => parse_size(f).unwrap_or_else(|| {
            eprintln!("bad --free");
            exit(2)
        }),
        None => free_target(&cache),
    };
    if target <= 0.0 {
        println!("enough free space; nothing to do (use --free SIZE to force a target)");
        return;
    }
    // a folder bigger than a quarter of the target is cut into its children, so part of it can be freed
    let split = target / 4.0;
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let items = reclaim::collect(&home, &cache, std::path::Path::new("/var/cache"), &mounts, discover::uid(), &a.roots, &browsers::running(), a.tau, bw, split);

    let size: Vec<f64> = items.iter().map(|i| i.size).collect();
    let loss: Vec<f64> = items.iter().map(|i| i.loss).collect();
    let prot: Vec<bool> = items.iter().map(|i| i.protected).collect();
    let mut skipped: BTreeMap<String, f64> = BTreeMap::new();
    for it in items.iter().filter(|i| i.protected && i.reason != scan::NOT_CACHE) {
        *skipped.entry(it.reason.clone()).or_default() += it.size;
    }
    let Some(p) = plan::plan(&size, &loss, &prot, target) else {
        println!("cannot free {} from rebuildable items", human(target));
        print_skipped(&skipped);
        exit(1)
    };
    let mut chosen: Vec<usize> = (0..items.len()).filter(|&i| p.chosen[i]).collect();
    chosen.sort_by(|&x, &y| items[y].size.partial_cmp(&items[x].size).unwrap());
    let mut total = 0.0;
    for &i in &chosen {
        println!("{:>8}  loss {:<8} {}", human(items[i].size), fmt_g(items[i].loss, 3), items[i].path.display());
        total += items[i].loss;
    }
    println!("frees {}, expected loss {}", human(p.freed), fmt_g(total, 3));
    print_skipped(&skipped);
    if !a.apply {
        return;
    }
    print!("delete these? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut ans = String::new();
    let _ = std::io::stdin().lock().read_line(&mut ans);
    if !matches!(ans.trim(), "y" | "Y") {
        println!("aborted");
        return;
    }
    let (_, errs) = reclaim::apply_items(&items, &chosen);
    for e in errs {
        eprintln!("{e}");
    }
    println!("done");
}

pub fn cache_root(home: &std::path::Path) -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".cache"))
}

/// Default target: bring the filesystem holding the cache to 15% free.
fn free_target(cache: &std::path::Path) -> f64 {
    let Some((total, avail)) = reclaim::disk(cache) else { return 0.0 };
    (0.15 * total - avail).max(0.0)
}

fn print_skipped(skipped: &BTreeMap<String, f64>) {
    for (why, b) in skipped {
        println!("skipped {}: {}", human(*b), why);
    }
}

/// `--plan SIZE`: items on stdin ("size loss protected"), prints `loss X` or `infeasible` (ampl/gap.py reads this).
fn plan_stdin(target: f64) -> i32 {
    let (mut size, mut loss, mut prot) = (Vec::new(), Vec::new(), Vec::new());
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 3 {
            continue;
        }
        let (Ok(s), Ok(l)) = (f[0].parse::<f64>(), f[1].parse::<f64>()) else { continue };
        size.push(s);
        loss.push(l);
        prot.push(f[2] != "0");
    }
    match plan::plan(&size, &loss, &prot, target) {
        None => {
            println!("infeasible");
            1
        }
        Some(p) => {
            let tot: f64 = (0..size.len()).filter(|&i| p.chosen[i]).map(|i| loss[i]).sum();
            println!("loss {}", fmt_g(tot, 9));
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_style_g_formatting() {
        // values the Go version printed on this machine
        for (v, want) in [(102.4, "102"), (73.9, "73.9"), (10.23, "10.2"), (0.00751, "0.00751"), (0.153, "0.153"), (348.0, "348"), (1000.0, "1e+03"), (1.0, "1"), (0.0001234, "0.000123"), (0.00001234, "1.23e-05")] {
            assert_eq!(fmt_g(v, 3), want, "{v}");
        }
        assert_eq!(fmt_g(1234.567890123, 9), "1234.56789");
    }

    #[test]
    fn sizes_and_args() {
        assert_eq!(parse_size("2G"), Some(2147483648.0));
        assert_eq!(parse_size("5M"), Some(5242880.0));
        assert_eq!(parse_size("1.5k"), Some(1536.0));
        assert_eq!(parse_size("x"), None);
        let a = parse_args(&["--free=2G".into(), "--root".into(), "/x".into(), "-apply".into(), "--tau".into(), "10".into()]).unwrap();
        assert_eq!((a.free.as_deref(), a.roots.len(), a.apply, a.tau), (Some("2G"), 1, true, 10.0));
        assert!(parse_args(&["--nope".into(), "1".into()]).is_err());
        assert!(parse_args(&["--free".into()]).is_err());
    }
}
