package main

import (
	"io/fs"
	"math"
	"os"
	"path/filepath"
	"syscall"
	"time"
)

// Item is one thing that could be freed: a top-level entry of a cache root, or a browser cache directory.
type Item struct {
	Path      string
	Size      float64
	Loss      float64
	Protected bool
	Reason    string // why it is protected, when it is
	Newest    time.Time
}

type inode struct{ dev, ino uint64 }

// measure returns the on-disk size (blocks*512, hard links counted once), the newest mtime/atime, and
// whether the tree carries a CACHEDIR.TAG. It stays on one filesystem and never follows symlinks.
func measure(root string) (size float64, newest time.Time, tagged bool) {
	seen := map[inode]bool{}
	var dev uint64
	if st, err := os.Lstat(root); err == nil {
		if s, ok := st.Sys().(*syscall.Stat_t); ok {
			dev = uint64(s.Dev)
		}
	}
	filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		info, err := d.Info()
		if err != nil {
			return nil
		}
		s, ok := info.Sys().(*syscall.Stat_t)
		if !ok {
			return nil
		}
		if uint64(s.Dev) != dev {
			if d.IsDir() {
				return filepath.SkipDir
			}
			return nil
		}
		if !info.IsDir() && s.Nlink > 1 {
			k := inode{uint64(s.Dev), uint64(s.Ino)}
			if seen[k] {
				return nil
			}
			seen[k] = true
		}
		size += float64(s.Blocks) * 512
		for _, t := range []time.Time{info.ModTime(), time.Unix(int64(s.Atim.Sec), int64(s.Atim.Nsec))} {
			if t.After(newest) {
				newest = t
			}
		}
		if d.Name() == "CACHEDIR.TAG" {
			tagged = true
		}
		return nil
	})
	return
}

// loss = exp(-age/tau) * (1 + size/bw): the chance it is needed again times the time to get it back.
func loss(size float64, newest time.Time, tauDays, bw float64) float64 {
	age := time.Since(newest).Hours() / 24
	if age < 0 {
		age = 0
	}
	return math.Exp(-age/tauDays) * (1 + size/bw)
}

// addItem measures one path and appends it. rebuildable = lives in a cache root or carries a CACHEDIR.TAG.
func addItem(items []Item, path string, rebuildable bool, tau, bw float64) []Item {
	size, newest, tagged := measure(path)
	if size <= 0 {
		return items
	}
	it := Item{Path: path, Size: size, Newest: newest, Loss: loss(size, newest, tau, bw)}
	if !(rebuildable || tagged) {
		it.Protected, it.Reason = true, "not a cache (no cache root, no CACHEDIR.TAG)"
	}
	return append(items, it)
}

// scanRoot adds every direct child of root.
func scanRoot(items []Item, root string, rebuildable bool, tau, bw float64) []Item {
	ents, err := os.ReadDir(root)
	if err != nil {
		return items
	}
	for _, e := range ents {
		items = addItem(items, filepath.Join(root, e.Name()), rebuildable, tau, bw)
	}
	return items
}
