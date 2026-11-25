package main

import (
	"math"
	"math/rand"
	"testing"
)

// brute force the exact optimum (no grid) for small n
func brute(size, loss []float64, prot []bool, target float64) float64 {
	best := math.Inf(1)
	n := len(size)
	for m := 0; m < 1<<n; m++ {
		s, l := 0.0, 0.0
		bad := false
		for i := 0; i < n; i++ {
			if m>>i&1 == 1 {
				if prot[i] {
					bad = true
					break
				}
				s += size[i]
				l += loss[i]
			}
		}
		if !bad && s >= target && l < best {
			best = l
		}
	}
	return best
}

func TestPlanProperties(t *testing.T) {
	rng := rand.New(rand.NewSource(1))
	for round := 0; round < 300; round++ {
		n := 3 + rng.Intn(10)
		size, loss, prot := make([]float64, n), make([]float64, n), make([]bool, n)
		total := 0.0
		for i := range size {
			size[i] = math.Exp(rng.NormFloat64()*1.5+16) + 1
			loss[i] = rng.ExpFloat64()*10 + 0.1
			prot[i] = rng.Intn(4) == 0
			if !prot[i] {
				total += size[i]
			}
		}
		target := total * (0.1 + 0.6*rng.Float64())
		chosen, freed, ok := Plan(size, loss, prot, target)
		opt := brute(size, loss, prot, target)
		if !ok {
			// the grid rounds sizes down, so it may refuse a target the exact solver reaches only barely
			if !math.IsInf(opt, 1) && total > target*1.05 {
				t.Fatalf("round %d: feasible with slack but planner refused", round)
			}
			continue
		}
		got, bytes := 0.0, 0.0
		for i, c := range chosen {
			if c {
				if prot[i] {
					t.Fatalf("round %d: protected item %d was planned", round, i)
				}
				got += loss[i]
				bytes += size[i]
			}
		}
		if bytes < target || math.Abs(bytes-freed) > 1 {
			t.Fatalf("round %d: freed %.0f < target %.0f", round, bytes, target)
		}
		if got < opt-1e-9 {
			t.Fatalf("round %d: loss %.4f below the exact optimum %.4f", round, got, opt)
		}
	}
}

func TestPlanInfeasibleWhenOnlyProtected(t *testing.T) {
	_, _, ok := Plan([]float64{1e9}, []float64{1}, []bool{true}, 1e6)
	if ok {
		t.Fatal("planned a protected item")
	}
}
