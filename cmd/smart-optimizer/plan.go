package main

import "math"

// K is the number of grid steps. lean4/Plan.lean holds for any K > 0; ampl/gap.py measures the gap to the optimum.
const K = 16384

// Plan picks the set of unprotected items that frees at least target bytes at minimum total loss
// (knapsack cover). Sizes are rounded DOWN to unit = ceil(target/K), so the real freed bytes are
// never below target (lean4/Plan.lean: grid_cover). ok is false when it is impossible.
func Plan(size, loss []float64, prot []bool, target float64) (chosen []bool, freed float64, ok bool) {
	n := len(size)
	chosen = make([]bool, n)
	unit := math.Ceil(target / K)
	if unit < 1 {
		return chosen, 0, false
	}
	f := make([]float64, K+1)
	for j := 1; j <= K; j++ {
		f[j] = math.Inf(1)
	}
	row := (K + 1 + 7) / 8
	take := make([]byte, n*row+1)
	weight := func(i int) int {
		w := math.Floor(size[i] / unit)
		if w > K {
			w = K
		}
		return int(w)
	}
	for i := 0; i < n; i++ {
		if prot[i] {
			continue
		}
		w := weight(i)
		if w < 1 {
			continue
		}
		for j := K; j >= 1; j-- {
			from := j - w
			if from < 0 {
				from = 0
			}
			if f[from]+loss[i] < f[j] {
				f[j] = f[from] + loss[i]
				take[i*row+j/8] |= 1 << (j % 8)
			}
		}
	}
	if math.IsInf(f[K], 1) {
		return chosen, 0, false
	}
	j := K
	for i := n - 1; i >= 0 && j > 0; i-- {
		if take[i*row+j/8]>>(j%8)&1 == 1 {
			chosen[i] = true
			freed += size[i]
			j -= weight(i)
			if j < 0 {
				j = 0
			}
		}
	}
	return chosen, freed, true
}
