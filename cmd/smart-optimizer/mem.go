package main

import (
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
)

// memReport only reads: /proc/meminfo, /proc/swaps, /sys/block/zram*, nvidia-smi when present.
// It says what could be switched off and why, and prints the command. Nothing is executed.
func memReport(w io.Writer) {
	mi := readKV("/proc/meminfo")
	total, avail := mi["MemTotal"], mi["MemAvailable"]
	fmt.Fprintf(w, "RAM   %s total, %s available, %s page cache\n", human(total), human(avail), human(mi["Cached"]))

	type swap struct {
		name       string
		size, used float64
		prio       int
	}
	var swaps []swap
	if b, err := os.ReadFile("/proc/swaps"); err == nil {
		for _, ln := range strings.Split(string(b), "\n")[1:] {
			f := strings.Fields(ln)
			if len(f) < 5 {
				continue
			}
			sz, _ := strconv.ParseFloat(f[2], 64)
			us, _ := strconv.ParseFloat(f[3], 64)
			pr, _ := strconv.Atoi(f[4])
			swaps = append(swaps, swap{f[0], sz * 1024, us * 1024, pr})
		}
	}
	var swapTotal, swapUsed float64
	for _, s := range swaps {
		swapTotal += s.size
		swapUsed += s.used
	}
	fmt.Fprintf(w, "swap  %s total, %s used, swappiness %s\n", human(swapTotal), human(swapUsed), readTrim("/proc/sys/vm/swappiness"))
	margin := total * 0.10
	for _, s := range swaps {
		verdict := "keep"
		switch {
		case s.used == 0:
			verdict = "idle: costs no RAM, only disk"
		case s.used < avail-margin:
			verdict = fmt.Sprintf("its %s would fit back into RAM: swapoff %s", human(s.used), s.name)
		default:
			verdict = "in use and RAM cannot take it back"
		}
		fmt.Fprintf(w, "      prio %-4d %8s used of %-8s %s  -> %s\n", s.prio, human(s.used), human(s.size), s.name, verdict)
	}
	if total > 0 && swapTotal > 2*total {
		fmt.Fprintf(w, "note  swap is %.1fx RAM; the idle part is only disk space\n", swapTotal/total)
	}
	zr, _ := filepath.Glob("/sys/block/zram*")
	for _, z := range zr {
		orig, mem := readNum(filepath.Join(z, "orig_data_size")), readNum(filepath.Join(z, "mem_used_total"))
		if orig > 0 {
			fmt.Fprintf(w, "zram  %s: %s of data held in %s of RAM (%.1fx)\n", filepath.Base(z), human(orig), human(mem), orig/maxf(mem, 1))
		}
	}
	if out, err := exec.Command("nvidia-smi", "--query-gpu=memory.used,memory.total", "--format=csv,noheader,nounits").Output(); err == nil {
		f := strings.Split(strings.TrimSpace(string(out)), ",")
		if len(f) == 2 {
			u, _ := strconv.ParseFloat(strings.TrimSpace(f[0]), 64)
			t, _ := strconv.ParseFloat(strings.TrimSpace(f[1]), 64)
			fmt.Fprintf(w, "VRAM  %s used of %s\n", human(u*1048576), human(t*1048576))
		}
		if apps, err := exec.Command("nvidia-smi", "--query-compute-apps=pid,used_memory,process_name", "--format=csv,noheader").Output(); err == nil {
			for _, ln := range strings.Split(strings.TrimSpace(string(apps)), "\n") {
				if ln != "" {
					fmt.Fprintf(w, "      compute: %s\n", ln)
				}
			}
		}
	} else {
		fmt.Fprintln(w, "VRAM  not checked (no nvidia-smi)")
	}
}

func readKV(p string) map[string]float64 {
	m := map[string]float64{}
	b, _ := os.ReadFile(p)
	for _, ln := range strings.Split(string(b), "\n") {
		k, v, ok := strings.Cut(ln, ":")
		if !ok {
			continue
		}
		f := strings.Fields(v)
		if len(f) == 0 {
			continue
		}
		n, _ := strconv.ParseFloat(f[0], 64)
		if len(f) > 1 && f[1] == "kB" {
			n *= 1024
		}
		m[strings.TrimSpace(k)] = n
	}
	return m
}

func readTrim(p string) string {
	b, err := os.ReadFile(p)
	if err != nil {
		return "?"
	}
	return strings.TrimSpace(string(b))
}

func readNum(p string) float64 {
	n, _ := strconv.ParseFloat(readTrim(p), 64)
	return n
}

func maxf(a, b float64) float64 {
	if a > b {
		return a
	}
	return b
}
