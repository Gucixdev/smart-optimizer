# smart-optimizer

One binary (Rust; Go kept as reference) that frees disk space at the least expected loss and reports what RAM, swap and VRAM are doing.
No config, no rule lists, no daemon. Nothing is changed without `--apply`.

Each candidate gets `loss = exp(-age/tau) * (1 + size/bw)`: the chance it is needed again times the time to get
it back. The planner picks the set that frees at least the target at minimum total loss (knapsack cover on a
16384-step grid). A candidate counts as rebuildable if it lives in a cache root or holds a `CACHEDIR.TAG`;
everything else is protected and never planned.

Where it looks is taken from standards and the mount table, not from program names: `$XDG_CACHE_HOME`, the FHS
`/var/cache` (needs root: those items are listed as skipped with the size, run with `sudo` to include them), the
freedesktop trash of the home and of every mounted disk (`.Trash-UID`, found from `/proc/mounts`; a trashed entry ages
from the time it was trashed and its `.trashinfo` goes with it). A folder bigger than a quarter of the target is cut
into its children (3 levels), so part of a 13 GB package cache can be freed instead of all or nothing.
`smart-optimizer where [DIR] [--min 1G]` shows what holds the space: a tree of everything bigger than `--min`, marked
cache or trash or neither with days since last touch, then the biggest and coldest things nobody can vouch for,
ranked by `size * (1 - exp(-age/tau))`. It plans nothing from that list.

Cache roots read today: `$XDG_CACHE_HOME` (default `~/.cache`), every Flatpak app cache (`~/.var/app/*/cache`),
the cache directories of Chromium-family browsers (`Cache`, `Code Cache`, `GPUCache`, `DawnCache`, shader caches;
Chrome, Chromium, Brave, Vivaldi, Edge, Opera, also the Flatpak ones), and any `--root DIR` (protected unless it
carries `CACHEDIR.TAG`). Firefox and others keep their cache under the XDG root already. Profile data (cookies,
logins, bookmarks, history, extensions, local storage, IndexedDB) is never a candidate. A browser with a live
process has its cache protected and reported as skipped.

    cd rust && cargo build --release          # the main version: target/release/smart-optimizer
    go build -o smart-optimizer ./cmd/smart-optimizer   # the Go version (reference, same interface and output)
    ./smart-optimizer                 # dry run, target = filesystem at 15% free
    ./smart-optimizer --free 5G       # explicit target
    ./smart-optimizer --root DIR      # extra root
    ./smart-optimizer --apply         # ask once, then delete the planned items
    ./smart-optimizer mem             # RAM, swap, zram, VRAM: read only, prints what could be switched off
    ./smart-optimizer where [DIR] [--min 1G]   # what holds the space (tree + cold unvouched things), read only
    ./smart-optimizer check [--below 15%]      # one line, read only; exit 0 enough free, 3 below, 2 bad use

`check` prints `disk: 14% free (16.6 GB) on /; reclaimable without asking: 24.8 GB (cache, trash, needs-root 13.6 GB)`
(free space of `/`; reclaimable = the unprotected cache and trash items the planner would see, needs-root counted
apart and not in the first number; a running browser's cache counts in neither). Meant for cron or judged: it
reads, never deletes, and runs once, no daemon.

`mem` only reads `/proc/meminfo`, `/proc/swaps`, `/sys/block/zram*` and `nvidia-smi`. It prints a verdict per swap
device (idle, would fit back into RAM, cannot) and the command, and runs none of them.

## How it was checked

| Piece | Where | Says |
|---|---|---|
| Rust tests | `rust/src/*.rs` (`cargo test`, clippy with `-D warnings`) | 16 tests, not run through `./check`. The newest ten are on fake folders under `temp_dir()`: mount and trash discovery, splitting, needs-root, the `where` tree and ranking, the `--apply` deletion (a cache, a trash with `files/` and `info/` and another disk's `.Trash-UID`: chosen items and their `.trashinfo` are gone, unchosen ones, a `--root` without a tag, a running browser's cache and Chromium profile data stay), a protected item is never deleted, a pacman package named `ca-certificates-mozilla` is not mistaken for Firefox, and the `check` line, exit code and sums. The first 6: the four of the Go tests (300 random instances against a brute-force optimum (covers the target, never plans a protected item, never beats the exact optimum), Chromium cache discovery on a fake home never lists profile data, a running browser's cache is protected, and an infeasible case) plus `%g` formatting and argument parsing |
| Go tests | `cmd/smart-optimizer/*_test.go` | 300 random instances against a brute-force optimum: covers the target, never plans a protected item, never beats the exact optimum; Chromium cache discovery on a fake home never lists profile data; a running browser's cache is protected |
| AMPL model + HiGHS | `ampl/` | the optimum; both the Rust and the Go planner are within 0.41% (p95), 0.97% (max) on the same 200 random instances (identical numbers, same algorithm) |
| Lean 4 | `lean4/Plan.lean` | rounding never under-frees; a protected item can never be planned (proves the algorithm, the Go code is held to it by the two rows above) |
| `./check` | | Rust: clippy, tests, release build; Go: vet, tests, build; Lean; then the AMPL comparison for the Rust binary and for the Go binary |

## Versions

- `rust/` is the main version (stdlib plus `libc` for `statvfs`; the binary name is set once, in `rust/Cargo.toml`). It
  passed `./check` (tests, clippy, Lean, AMPL gap) and, on this machine, printed byte-for-byte the same output as the
  Go version for `--free 2G` (18 lines, same items, same freed size, same loss) and for `mem`. Warm cache, that
  dry-run took about 0.5 s in Rust and 0.8 to 1.5 s in Go; the cold first run (disk) took 53 s, so the time is mostly the disk.
- `cmd/smart-optimizer/` (Go) is the reference it was ported from; same flags, same `--plan` stdin format. Kept, not removed.
- `reference/smart-cleaner.c` is the first version (C, disk only).

## TODO

Done:
- [x] Planner: exact grid DP, Lean-proved cover, held to the AMPL optimum (`./check`)
- [x] Hard links counted once
- [x] Browser caches (Chromium family and Flatpak) and the running-browser guard
- [x] Rust port (`rust/`), held to the same AMPL optimum and the same tests as Go
- [x] `mem`: read-only report of RAM, swap, zram, VRAM
- [x] Discovery by standard, not by program name: XDG cache, FHS `/var/cache` (needs root, reported), freedesktop trash of the home and of every mount; big folders cut into children; `where`
- [x] `--apply` tested: the deletion code is `reclaim::apply_items`, covered by Rust tests on fake homes, and the real binary was run with `--apply` (answering `n`, then `y`) on a fake home under `/var/tmp/so-test` with `HOME` and `XDG_CACHE_HOME` pointing there; right items removed, `.trashinfo` removed with its file, protected items stayed. Bug found on the way and fixed: a browser mark matched by substring protected pacman's `ca-certificates-mozilla-*.pkg.tar.zst` while Firefox ran (and hid it from the needs-root sum); marks now match whole path components
- [x] `check [--below 15%]` (exit 0/3), installed to `~/.local/bin/smart-optimizer` on this machine; run for real read-only against the real disk (exit 3 at 15%: 14% free)

Not done (each is a real gap, nothing here is checked):
- [ ] Browser caches were tested only on a fake home and with Firefox (Flatpak) on this machine; real Chrome, Chromium, Brave, Vivaldi, Edge, Opera, snap and Windows/macOS paths are not tried
- [ ] Duplicates, each an option you switch on: byte-for-byte (content hash), and look-alike images (perceptual hash: resized, recompressed, blurred); list only by default, never auto-delete, keep one by a rule you set
- [ ] Junk found by logic rather than by name: `where` lists cold, big, unvouched folders but the planner never takes them; a rule that does (say, a folder untouched for 1 year and recreatable from a lockfile or a git remote) is not designed. Also not done: old journal and coredumps, orphaned Flatpak data. `--apply` as root was never run
- [ ] `mem --apply`: switch swap and zram on and off, tune `vm.swappiness`, drop caches only under pressure (needs root; the decision rule is not written or proved)
- [ ] VRAM: list who holds it (done for compute apps via nvidia-smi); freeing it is not implemented
- [ ] Game mode like gamescope: on game start stop what is not needed, restore afterwards (needs an allow list and a restore log; not designed yet)
- [ ] Processes: nothing is stopped; a stop list must be explicit and reversible
- [ ] Backup service (moved here from judged on 2026-11-25: making backups and testing restores are actions, judged only reads evidence): restore one file from the newest snapshot into a scratch folder, compare its hash, write a stamp judged reads (`restore_tested_file`, one line `YYYY-MM-DD`, written only when the hash matched); then the backup run itself (restic or borg as the engine, nothing of our own), retention by rule, never deleting the last good snapshot
- [ ] Items smaller than one grid step are never planned
- [ ] `tau` (30 days) and `bw` (5 MB/s) are assumed, not measured
- [ ] State file so a rerun does not rescan everything (a scan of 9 GB of `~/.cache` took 28 s here)
- [ ] `--yes` with a safety threshold (refuse to delete more than a set share without asking)
- [ ] `--apply` against a real home, real trash on another disk, or as root was never run (only the fake home above; the real binary also scans the real `/var/cache` and `/proc/mounts`, so that run was done only after the dry run showed every planned path inside the fake home)
- [ ] Windows and macOS scanners (Linux only today: `MetadataExt`/`statvfs`)
- [ ] The Go version was not updated for `where`, `check`, trash/`/var/cache` discovery or splitting (Rust is the main version), so the "same output" claim holds only for the older behaviour
- [ ] `check` reads `/` only and sums items from the home, whatever disk the home is on; no `--path`; the planner grid rounds sizes down, so a target above about 95% of all reclaimable bytes is reported infeasible even if it is exactly reachable
- [ ] The Rust version was compared with Go on one machine and one cache (identical output); other machines, Flatpak overrides of the cache location and unusual `XDG_CACHE_HOME` are not tried
- [ ] Final name of the tool is not chosen; it lives in one place (`rust/Cargo.toml`, plus folder and README titles)
- [ ] Put the code in a git repo and push (not done)
