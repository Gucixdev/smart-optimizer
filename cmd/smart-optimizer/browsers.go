package main

import (
	"os"
	"path/filepath"
	"strings"
)

// Browsers: only HTTP, code and shader caches are ever planned. Never profile data: cookies, logins, bookmarks,
// history, extensions, local storage, IndexedDB, sessions. Chromium-family profiles keep their caches under
// ~/.config/<browser>/<profile>/ next to that data, so the cache directory names are listed, nothing else.
var chromiumCacheDirs = []string{"Cache", "Code Cache", "GPUCache", "DawnCache", "GrShaderCache", "ShaderCache", "GraphiteDawnCache"}

type browser struct {
	name  string
	procs []string // /proc/*/comm names that mean it is running
	marks []string // a path containing one of these belongs to this browser
}

var browsers = []browser{
	{"firefox", []string{"firefox", "firefox-bin", "Isolated Web Co"}, []string{"mozilla", "firefox", "librewolf", "waterfox"}},
	{"chrome", []string{"chrome", "google-chrome"}, []string{"google-chrome", "com.google.Chrome"}},
	{"chromium", []string{"chromium", "chromium-browser"}, []string{"chromium", "org.chromium"}},
	{"brave", []string{"brave", "brave-browser"}, []string{"BraveSoftware", "brave"}},
	{"vivaldi", []string{"vivaldi-bin", "vivaldi"}, []string{"vivaldi"}},
	{"edge", []string{"msedge", "microsoft-edge"}, []string{"microsoft-edge", "com.microsoft.Edge"}},
	{"opera", []string{"opera"}, []string{"opera"}},
}

// browserCacheDirs finds Chromium-family cache directories that live outside the XDG cache root.
func browserCacheDirs(home string) []string {
	var out []string
	cfgs := []string{filepath.Join(home, ".config")}
	if apps, _ := filepath.Glob(filepath.Join(home, ".var/app/*/config")); apps != nil {
		cfgs = append(cfgs, apps...)
	}
	for _, cfg := range cfgs {
		for _, prof := range globProfiles(cfg) {
			for _, d := range chromiumCacheDirs {
				p := filepath.Join(prof, d)
				if st, err := os.Stat(p); err == nil && st.IsDir() {
					out = append(out, p)
				}
			}
		}
	}
	return out
}

// globProfiles returns <cfg>/<browser>/<profile> directories holding a Chromium "Preferences" file.
func globProfiles(cfg string) []string {
	var out []string
	for _, depth := range []string{"*/*/Preferences", "*/*/*/Preferences"} {
		m, _ := filepath.Glob(filepath.Join(cfg, depth))
		for _, p := range m {
			out = append(out, filepath.Dir(p))
		}
	}
	return out
}

// runningBrowsers lists browsers with a live process (read from /proc/*/comm).
func runningBrowsers() map[string]bool {
	run := map[string]bool{}
	comms, _ := filepath.Glob("/proc/[0-9]*/comm")
	for _, c := range comms {
		b, err := os.ReadFile(c)
		if err != nil {
			continue
		}
		name := strings.TrimSpace(string(b))
		for _, br := range browsers {
			for _, p := range br.procs {
				if name == p {
					run[br.name] = true
				}
			}
		}
	}
	return run
}

// guardRunning protects items of a browser that is running right now: its cache is open and locked.
func guardRunning(items []Item, running map[string]bool) []Item {
	for i := range items {
		for _, br := range browsers {
			if !running[br.name] {
				continue
			}
			for _, m := range br.marks {
				if strings.Contains(items[i].Path, m) {
					items[i].Protected, items[i].Reason = true, br.name+" is running"
				}
			}
		}
	}
	return items
}
