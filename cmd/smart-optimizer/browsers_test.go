package main

import (
	"os"
	"path/filepath"
	"testing"
)

func mk(t *testing.T, p string, file bool) {
	t.Helper()
	if file {
		os.MkdirAll(filepath.Dir(p), 0o755)
		os.WriteFile(p, []byte("x"), 0o644)
		return
	}
	os.MkdirAll(p, 0o755)
}

func TestChromiumCachesOnlyNeverProfileData(t *testing.T) {
	home := t.TempDir()
	prof := filepath.Join(home, ".config/google-chrome/Default")
	mk(t, filepath.Join(prof, "Preferences"), true)
	for _, d := range []string{"Cache", "Code Cache", "GPUCache", "Local Storage", "IndexedDB", "Extensions"} {
		mk(t, filepath.Join(prof, d), false)
	}
	mk(t, filepath.Join(prof, "Cookies"), true)
	mk(t, filepath.Join(prof, "Login Data"), true)
	flat := filepath.Join(home, ".var/app/org.chromium.Chromium/config/chromium/Default")
	mk(t, filepath.Join(flat, "Preferences"), true)
	mk(t, filepath.Join(flat, "GPUCache"), false)

	got := map[string]bool{}
	for _, p := range browserCacheDirs(home) {
		got[p] = true
	}
	for _, want := range []string{"Cache", "Code Cache", "GPUCache"} {
		if !got[filepath.Join(prof, want)] {
			t.Errorf("missing %s", want)
		}
	}
	if !got[filepath.Join(flat, "GPUCache")] {
		t.Error("flatpak Chromium cache not found")
	}
	for _, never := range []string{"Local Storage", "IndexedDB", "Extensions", "Cookies", "Login Data"} {
		if got[filepath.Join(prof, never)] {
			t.Errorf("profile data %s must never be a candidate", never)
		}
	}
}

func TestRunningBrowserCacheIsProtected(t *testing.T) {
	items := []Item{{Path: "/h/.cache/mozilla/firefox/x/cache2"}, {Path: "/h/.cache/google-chrome/Default/Cache"}, {Path: "/h/.cache/zig"}}
	items = guardRunning(items, map[string]bool{"firefox": true})
	if !items[0].Protected || items[1].Protected || items[2].Protected {
		t.Fatalf("only the running browser's cache is protected: %+v", items)
	}
}
