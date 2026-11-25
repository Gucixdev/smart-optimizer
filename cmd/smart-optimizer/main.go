// smart-optimizer: free disk space at the least expected loss, and report what RAM, swap and VRAM are doing.
// One binary, no config, no daemon, nothing is changed without --apply.
//
//	smart-optimizer [--free SIZE] [--tau DAYS] [--bw BYTES/S] [--root DIR]... [--apply]
//	smart-optimizer mem
//	smart-optimizer --plan SIZE        (stdin: "size loss protected" per line; prints the loss of the plan)
package main

import (
	"bufio"
	"flag"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"syscall"
)

type roots []string

func (r *roots) String() string     { return strings.Join(*r, ",") }
func (r *roots) Set(s string) error { *r = append(*r, s); return nil }

func parseSize(s string) (float64, error) {
	s = strings.TrimSpace(s)
	mult := 1.0
	if n := len(s); n > 0 {
		switch s[n-1] {
		case 'k', 'K':
			mult = 1 << 10
		case 'm', 'M':
			mult = 1 << 20
		case 'g', 'G':
			mult = 1 << 30
		case 't', 'T':
			mult = 1 << 40
		}
		if mult != 1 {
			s = s[:n-1]
		}
	}
	v, err := strconv.ParseFloat(s, 64)
	return v * mult, err
}

func human(b float64) string {
	u := []string{"B", "K", "M", "G", "T"}
	i := 0
	for b >= 1024 && i < len(u)-1 {
		b /= 1024
		i++
	}
	return fmt.Sprintf("%.1f%s", b, u[i])
}

func main() {
	if len(os.Args) > 1 && os.Args[1] == "mem" {
		memReport(os.Stdout)
		return
	}
	var (
		free, planSize string
		tau            = flag.Float64("tau", 30, "days: the reuse chance halves every tau*ln2 days")
		bw             = flag.String("bw", "5M", "bytes/s used to price re-downloading")
		apply          = flag.Bool("apply", false, "ask once, then delete the planned items")
		extra          roots
	)
	flag.StringVar(&free, "free", "", "free at least SIZE (default: bring the filesystem to 15% free)")
	flag.StringVar(&planSize, "plan", "", "planner only: read items from stdin")
	flag.Var(&extra, "root", "extra root; its children are protected unless they carry CACHEDIR.TAG")
	flag.Parse()
	bandwidth, err := parseSize(*bw)
	if err != nil || bandwidth <= 0 {
		fmt.Fprintln(os.Stderr, "bad --bw")
		os.Exit(2)
	}

	if planSize != "" {
		target, err := parseSize(planSize)
		if err != nil {
			fmt.Fprintln(os.Stderr, "bad --plan")
			os.Exit(2)
		}
		os.Exit(planStdin(target))
	}

	home, err := os.UserHomeDir()
	if err != nil {
		fmt.Fprintln(os.Stderr, "no HOME")
		os.Exit(2)
	}
	cache := os.Getenv("XDG_CACHE_HOME")
	if cache == "" {
		cache = filepath.Join(home, ".cache")
	}
	var items []Item
	items = scanRoot(items, cache, true, *tau, bandwidth)
	if apps, _ := filepath.Glob(filepath.Join(home, ".var/app/*/cache")); apps != nil {
		for _, a := range apps { // flatpak: an app's XDG_CACHE_HOME
			items = scanRoot(items, a, true, *tau, bandwidth)
		}
	}
	for _, d := range browserCacheDirs(home) {
		items = addItem(items, d, true, *tau, bandwidth)
	}
	for _, r := range extra {
		items = scanRoot(items, r, false, *tau, bandwidth)
	}
	items = guardRunning(items, runningBrowsers())

	target := 0.0
	if free != "" {
		if target, err = parseSize(free); err != nil {
			fmt.Fprintln(os.Stderr, "bad --free")
			os.Exit(2)
		}
	} else {
		var v syscall.Statfs_t
		if syscall.Statfs(cache, &v) == nil {
			total, avail := float64(v.Blocks)*float64(v.Bsize), float64(v.Bavail)*float64(v.Bsize)
			target = math.Max(0.15*total-avail, 0)
		}
	}
	if target <= 0 {
		fmt.Println("enough free space; nothing to do (use --free SIZE to force a target)")
		return
	}

	size, loss, prot := make([]float64, len(items)), make([]float64, len(items)), make([]bool, len(items))
	for i, it := range items {
		size[i], loss[i], prot[i] = it.Size, it.Loss, it.Protected
	}
	chosen, freed, ok := Plan(size, loss, prot, target)
	skipped := map[string]float64{}
	for _, it := range items {
		if it.Protected && strings.HasSuffix(it.Reason, "is running") {
			skipped[it.Reason] += it.Size
		}
	}
	if !ok {
		fmt.Printf("cannot free %s from rebuildable items\n", human(target))
		printSkipped(skipped)
		os.Exit(1)
	}
	var plan []int
	for i, c := range chosen {
		if c {
			plan = append(plan, i)
		}
	}
	sort.Slice(plan, func(a, b int) bool { return items[plan[a]].Size > items[plan[b]].Size })
	total := 0.0
	for _, i := range plan {
		fmt.Printf("%8s  loss %-8.3g %s\n", human(items[i].Size), items[i].Loss, items[i].Path)
		total += items[i].Loss
	}
	fmt.Printf("frees %s, expected loss %.3g\n", human(freed), total)
	printSkipped(skipped)
	if !*apply {
		return
	}
	fmt.Print("delete these? [y/N] ")
	ans, _ := bufio.NewReader(os.Stdin).ReadString('\n')
	if a := strings.TrimSpace(ans); a != "y" && a != "Y" {
		fmt.Println("aborted")
		return
	}
	for _, i := range plan {
		if items[i].Protected {
			continue
		}
		if err := os.RemoveAll(items[i].Path); err != nil {
			fmt.Fprintf(os.Stderr, "%s: %v\n", items[i].Path, err)
		}
	}
	fmt.Println("done")
}

func printSkipped(skipped map[string]float64) {
	for why, b := range skipped {
		fmt.Printf("skipped %s of cache: %s\n", human(b), why)
	}
}

func planStdin(target float64) int {
	var size, loss []float64
	var prot []bool
	sc := bufio.NewScanner(os.Stdin)
	for sc.Scan() {
		f := strings.Fields(sc.Text())
		if len(f) != 3 {
			continue
		}
		s, e1 := strconv.ParseFloat(f[0], 64)
		l, e2 := strconv.ParseFloat(f[1], 64)
		if e1 != nil || e2 != nil {
			continue
		}
		size, loss, prot = append(size, s), append(loss, l), append(prot, f[2] != "0")
	}
	chosen, _, ok := Plan(size, loss, prot, target)
	if !ok {
		fmt.Println("infeasible")
		return 1
	}
	tot := 0.0
	for i, c := range chosen {
		if c {
			tot += loss[i]
		}
	}
	fmt.Printf("loss %.9g\n", tot)
	return 0
}
