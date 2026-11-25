//! Knapsack cover: the set of unprotected items that frees at least `target` bytes at minimum total loss.
//! Sizes are rounded DOWN to unit = ceil(target/K), so the real freed bytes are never below target
//! (lean4/Plan.lean: grid_cover). Same algorithm as the Go and C versions.

/// Grid steps. lean4/Plan.lean holds for any K > 0; ampl/gap.py measures the gap to the optimum.
pub const K: usize = 16384;

pub struct Planned {
    pub chosen: Vec<bool>,
    pub freed: f64,
}

/// None when it is impossible.
pub fn plan(size: &[f64], loss: &[f64], prot: &[bool], target: f64) -> Option<Planned> {
    let n = size.len();
    let unit = (target / K as f64).ceil();
    if unit.is_nan() || unit < 1.0 {
        return None;
    }
    let weight = |i: usize| -> usize { (size[i] / unit).floor().min(K as f64) as usize };
    let mut f = vec![f64::INFINITY; K + 1];
    f[0] = 0.0;
    let row = (K + 1).div_ceil(8);
    let mut take = vec![0u8; n * row + 1];
    for i in 0..n {
        if prot[i] {
            continue;
        }
        let w = weight(i);
        if w < 1 {
            continue;
        }
        for j in (1..=K).rev() {
            let from = j.saturating_sub(w);
            if f[from] + loss[i] < f[j] {
                f[j] = f[from] + loss[i];
                take[i * row + j / 8] |= 1 << (j % 8);
            }
        }
    }
    if f[K].is_infinite() {
        return None;
    }
    let mut chosen = vec![false; n];
    let mut freed = 0.0;
    let mut j = K;
    for i in (0..n).rev() {
        if j == 0 {
            break;
        }
        if take[i * row + j / 8] >> (j % 8) & 1 == 1 {
            chosen[i] = true;
            freed += size[i];
            j = j.saturating_sub(weight(i));
        }
    }
    Some(Planned { chosen, freed })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64*: enough randomness for property tests, no dependency
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        fn normal(&mut self) -> f64 {
            let (a, b) = (self.unit().max(1e-12), self.unit());
            (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()
        }
    }

    /// the exact optimum without any grid, for small n
    fn brute(size: &[f64], loss: &[f64], prot: &[bool], target: f64) -> f64 {
        let n = size.len();
        let mut best = f64::INFINITY;
        'mask: for m in 0..1u32 << n {
            let (mut s, mut l) = (0.0, 0.0);
            for i in 0..n {
                if m >> i & 1 == 1 {
                    if prot[i] {
                        continue 'mask;
                    }
                    s += size[i];
                    l += loss[i];
                }
            }
            if s >= target && l < best {
                best = l;
            }
        }
        best
    }

    #[test]
    fn properties_against_brute_force() {
        let mut rng = Rng(0x9E3779B97F4A7C15);
        for round in 0..300 {
            let n = 3 + (rng.next() % 10) as usize;
            let mut size = vec![0.0; n];
            let mut loss = vec![0.0; n];
            let mut prot = vec![false; n];
            let mut total = 0.0;
            for i in 0..n {
                size[i] = (rng.normal() * 1.5 + 16.0).exp() + 1.0;
                loss[i] = -rng.unit().max(1e-12).ln() * 10.0 + 0.1;
                prot[i] = rng.next().is_multiple_of(4);
                if !prot[i] {
                    total += size[i];
                }
            }
            let target = total * (0.1 + 0.6 * rng.unit());
            let opt = brute(&size, &loss, &prot, target);
            match plan(&size, &loss, &prot, target) {
                None => {
                    // the grid rounds sizes down, so it may refuse a target the exact solver reaches only barely
                    assert!(!(opt.is_finite() && total > target * 1.05), "round {round}: feasible with slack but refused");
                }
                Some(p) => {
                    let (mut got, mut bytes) = (0.0, 0.0);
                    for i in 0..n {
                        if p.chosen[i] {
                            assert!(!prot[i], "round {round}: protected item {i} was planned");
                            got += loss[i];
                            bytes += size[i];
                        }
                    }
                    assert!(bytes >= target && (bytes - p.freed).abs() <= 1.0, "round {round}: freed {bytes} < target {target}");
                    assert!(got >= opt - 1e-9, "round {round}: loss {got} below the exact optimum {opt}");
                }
            }
        }
    }

    #[test]
    fn infeasible_when_only_protected() {
        assert!(plan(&[1e9], &[1.0], &[true], 1e6).is_none());
    }
}
