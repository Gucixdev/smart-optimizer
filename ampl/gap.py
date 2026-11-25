"""Heuristic vs AMPL optimum on random instances. Run: ~/.local/share/ampl/bin/python ampl/gap.py [N] [path/to/smart-cleaner binary]
The same instances are later fed to the C binary (`smart-cleaner --plan`) and held to this gap."""
import math, random, subprocess, sys
from amplpy import AMPL, modules

TAU = 30.0  # days: reuse probability halves every TAU*ln2 days

def instance(rng, n):
    items = []
    for _ in range(n):
        size = math.exp(rng.gauss(16, 2.5))               # bytes, log-normal: many small, few huge
        age = rng.expovariate(1 / 60)                      # days since last access
        prot = rng.random() < 0.25                         # not a cache: cannot be rebuilt
        rebuild = 1.0 + size / 5e6                          # seconds: fixed cost + re-download at 5 MB/s
        items.append((size, math.exp(-age / TAU) * rebuild, prot))
    return items

def greedy(items, target):
    """Ratio greedy (loss per byte, ascending); the last gap is closed by the cheapest single item that covers it
    or by one more greedy step, whichever costs less; then redundant items are dropped (reverse delete)."""
    free = sorted((l / s, s, l) for s, l, p in items if not p)
    best = math.inf
    got = cost = 0.0
    chosen = []
    for k, (_, s, l) in enumerate(free):
        if got >= target: break
        need = target - got
        close = min((l2 for _, s2, l2 in free[k:] if s2 >= need), default=math.inf)   # one item finishes the job
        best = min(best, cost + close)
        got += s; cost += l; chosen.append((s, l))
    if got >= target:
        for s, l in sorted(chosen, key=lambda c: -c[1]):                                # drop what is not needed
            if got - s >= target: got -= s; cost -= l
        best = min(best, cost)
    return None if best == math.inf else best

def dp(items, target, K=4096):
    """Exact on a grid: sizes rounded DOWN to target/K units, so the true freed bytes always cover the target.
    best[j] = least loss to free at least j units. O(n*K), no solver, no rules."""
    unit = target / K
    best = [math.inf] * (K + 1); best[0] = 0.0
    for s, l, p in items:
        if p: continue
        w = int(s / unit)
        if w == 0: continue
        for j in range(K, -1, -1):                      # 0/1: walk down so an item is used once
            if best[j] < math.inf or j == 0:
                k = min(K, j + w)
                if best[j] + l < best[k]: best[k] = best[j] + l
    return None if best[K] == math.inf else best[K]

def c_binary(binary, items, target):
    """Run the shipped C planner on the same instance (stdin: size loss protected)."""
    text = "".join(f"{s!r} {l!r} {int(p)}\n" for s, l, p in items)
    r = subprocess.run([binary, "--plan", repr(target)], input=text, capture_output=True, text=True)
    return float(r.stdout.split()[1]) if r.returncode == 0 else None

def optimum(ampl, items, target):
    ampl.reset()
    ampl.read("ampl/clean.mod")
    ampl.set["ITEMS"] = list(range(len(items)))
    ampl.param["size"] = {i: s for i, (s, l, p) in enumerate(items)}
    ampl.param["loss"] = {i: l for i, (s, l, p) in enumerate(items)}
    ampl.param["protected"] = {i: int(p) for i, (s, l, p) in enumerate(items)}
    ampl.param["target"] = target
    ampl.option["solver"] = "highs"
    ampl.option["highs_options"] = "mipgap=0"
    ampl.solve()
    if ampl.get_value("solve_result") != "solved": return None
    return ampl.get_objective("expected_loss").value()

if __name__ == "__main__":
    modules.load()
    ampl = AMPL()
    rng = random.Random(1)
    runs = int(sys.argv[1]) if len(sys.argv) > 1 else 100
    gaps = []
    for _ in range(runs):
        items = instance(rng, rng.choice([20, 50, 100]))
        total = sum(s for s, l, p in items if not p)
        target = total * rng.choice([0.1, 0.3, 0.6])
        opt = optimum(ampl, items, target)
        g = c_binary(sys.argv[2], items, target) if len(sys.argv) > 2 else dp(items, target)
        if opt is None or g is None or opt == 0: continue
        gaps.append(g / opt)
    gaps.sort()
    print(f"instances {len(gaps)}  gap median {gaps[len(gaps)//2]:.4f}  p95 {gaps[int(len(gaps)*.95)]:.4f}  max {gaps[-1]:.4f}  (1.0 = optimal)")
