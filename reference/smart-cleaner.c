/* smart-cleaner: free disk space at the least expected loss.
 *
 * Every candidate gets loss = exp(-age/tau) * (1 + size/bw): the chance it is needed again times the
 * time to get it back. The planner picks the set that frees >= target at minimum total loss (knapsack
 * cover, exact on a 4096-step grid). No rule lists: a candidate is rebuildable if it lives in a cache
 * root or carries a CACHEDIR.TAG; everything else is protected and never planned.
 * Proofs: lean4/Plan.lean (rounding never under-frees, protected never planned). Optimum check: ampl/.
 *
 * usage: smart-cleaner [--free SIZE] [--tau DAYS] [--bw BYTES/S] [--root DIR]... [--apply]
 *        smart-cleaner --plan SIZE   (stdin: "size loss protected" per line; stdout: chosen line numbers)
 * Default target: bring the filesystem to 15% free. Without --apply nothing is touched. */
#define _XOPEN_SOURCE 700
#include <ftw.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <time.h>
#include <dirent.h>
#include <unistd.h>

#ifndef K
#define K 16384 /* grid steps; Plan.lean holds for any K > 0; measured vs AMPL in ampl/gap.py */
#endif

typedef struct { char *path; double size, loss; int prot; time_t newest; } Item;

static double parse_size(const char *s)
{
	char *e;
	double v = strtod(s, &e);
	switch (*e) {
	case 'k': case 'K': v *= 1024; break;
	case 'm': case 'M': v *= 1024.0 * 1024; break;
	case 'g': case 'G': v *= 1024.0 * 1024 * 1024; break;
	case 't': case 'T': v *= 1024.0 * 1024 * 1024 * 1024; break;
	}
	return v;
}

/* Minimum total loss for freeing >= target. chosen[i] = 1 for planned items. Returns 0 if impossible.
 * Sizes are rounded DOWN to unit = ceil(target/K): real freed bytes >= target (Plan.lean: grid_cover). */
static int plan(const double *size, const double *loss, const int *prot, int n, double target, char *chosen, double *freed)
{
	double unit = ceil(target / K);
	double *f = malloc((K + 1) * sizeof *f);
	size_t row = (K + 1 + 7) / 8;
	uint8_t *take = calloc((size_t)n * row + 1, 1);
	int j, i, ok;
	if (!f || !take) { free(f); free(take); return 0; }
	f[0] = 0;
	for (j = 1; j <= K; j++) f[j] = INFINITY;
	for (i = 0; i < n; i++) {
		double w;
		chosen[i] = 0;
		if (prot[i] || unit < 1) continue;
		w = floor(size[i] / unit);
		if (w < 1) continue;
		for (j = K; j >= 1; j--) {
			int from = j - (w > j ? j : (int)w);
			if (f[from] + loss[i] < f[j]) { f[j] = f[from] + loss[i]; take[(size_t)i * row + j / 8] |= 1 << (j % 8); }
		}
	}
	ok = f[K] < INFINITY;
	*freed = 0;
	if (ok) {
		j = K;
		for (i = n - 1; i >= 0 && j > 0; i--) {
			if (take[(size_t)i * row + j / 8] >> (j % 8) & 1) {
				double w = floor(size[i] / unit);
				chosen[i] = 1; *freed += size[i];
				j -= w > j ? j : (int)w;
			}
		}
	}
	free(f); free(take);
	return ok;
}

/* ---- scanning (POSIX nftw) ---- */
static double tree_size; static time_t tree_newest; static int tree_tag;
static int visit(const char *p, const struct stat *st, int flag, struct FTW *ft)
{
	(void)flag;
	tree_size += (double)st->st_blocks * 512;
	if (st->st_mtime > tree_newest) tree_newest = st->st_mtime;
	if (st->st_atime > tree_newest) tree_newest = st->st_atime;
	if (!tree_tag && strcmp(p + ft->base, "CACHEDIR.TAG") == 0) tree_tag = 1;
	return 0;
}

static Item *items; static int n_items, cap_items;
static void add_item(const char *path, int in_cache, double tau, double bw)
{
	time_t now = time(NULL);
	double age, rebuild;
	Item *it;
	tree_size = 0; tree_newest = 0; tree_tag = 0;
	nftw(path, visit, 32, FTW_PHYS | FTW_MOUNT);
	if (tree_size <= 0) return;
	if (n_items == cap_items) { cap_items = cap_items ? cap_items * 2 : 256; items = realloc(items, cap_items * sizeof *items); }
	it = &items[n_items++];
	it->path = strdup(path); it->size = tree_size; it->newest = tree_newest;
	it->prot = !(in_cache || tree_tag);
	age = difftime(now, tree_newest) / 86400.0; if (age < 0) age = 0;
	rebuild = 1.0 + tree_size / bw;
	it->loss = exp(-age / tau) * rebuild;
}

static void scan_root(const char *root, int in_cache, double tau, double bw)
{
	DIR *d = opendir(root);
	struct dirent *e;
	if (!d) return;
	while ((e = readdir(d))) {
		char path[4096];
		if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..")) continue;
		snprintf(path, sizeof path, "%s/%s", root, e->d_name);
		add_item(path, in_cache, tau, bw);
	}
	closedir(d);
}

static int rm_cb(const char *p, const struct stat *st, int flag, struct FTW *ft)
{ (void)st; (void)flag; (void)ft; return remove(p); }

static void human(double b, char *out)
{
	const char *u[] = {"B", "K", "M", "G", "T"}; int i = 0;
	while (b >= 1024 && i < 4) { b /= 1024; i++; }
	sprintf(out, "%.1f%s", b, u[i]);
}

int main(int argc, char **argv)
{
	double target = -1, tau = 30, bw = 5e6, freed;
	char *roots[16]; int nroots = 0, apply = 0, i, plan_mode = 0;
	const char *home = getenv("HOME"), *xdg = getenv("XDG_CACHE_HOME");
	char defroot[4096], *chosen = NULL; double *sz = NULL, *ls = NULL; int *pr = NULL;

	for (i = 1; i < argc; i++) {
		if (!strcmp(argv[i], "--free") && i + 1 < argc) target = parse_size(argv[++i]);
		else if (!strcmp(argv[i], "--tau") && i + 1 < argc) tau = atof(argv[++i]);
		else if (!strcmp(argv[i], "--bw") && i + 1 < argc) bw = parse_size(argv[++i]);
		else if (!strcmp(argv[i], "--root") && i + 1 < argc && nroots < 16) roots[nroots++] = argv[++i];
		else if (!strcmp(argv[i], "--apply")) apply = 1;
		else if (!strcmp(argv[i], "--plan") && i + 1 < argc) { plan_mode = 1; target = parse_size(argv[++i]); }
		else { fputs("usage: smart-cleaner [--free SIZE] [--tau DAYS] [--bw B/S] [--root DIR] [--apply]\n", stderr); return 2; }
	}

	if (plan_mode) { /* stdin: size loss protected */
		int cap = 1024, n = 0; double s, l; int p;
		sz = malloc(cap * sizeof *sz); ls = malloc(cap * sizeof *ls); pr = malloc(cap * sizeof *pr);
		while (scanf("%lf %lf %d", &s, &l, &p) == 3) {
			if (n == cap) { cap *= 2; sz = realloc(sz, cap * sizeof *sz); ls = realloc(ls, cap * sizeof *ls); pr = realloc(pr, cap * sizeof *pr); }
			sz[n] = s; ls[n] = l; pr[n] = p; n++;
		}
		chosen = malloc(n + 1);
		if (!plan(sz, ls, pr, n, target, chosen, &freed)) { puts("infeasible"); return 1; }
		{ double tot = 0; for (i = 0; i < n; i++) if (chosen[i]) tot += ls[i]; printf("loss %.9g\n", tot); }
		return 0;
	}

	if (xdg && *xdg) snprintf(defroot, sizeof defroot, "%s", xdg);
	else if (home) snprintf(defroot, sizeof defroot, "%s/.cache", home);
	else { fputs("no HOME\n", stderr); return 2; }
	scan_root(defroot, 1, tau, bw);
	for (i = 0; i < nroots; i++) scan_root(roots[i], 0, tau, bw);

	if (target < 0) { /* default: bring the filesystem holding the cache to 15% free */
		struct statvfs v;
		if (statvfs(defroot, &v) == 0) {
			double total = (double)v.f_blocks * v.f_frsize, avail = (double)v.f_bavail * v.f_frsize;
			target = 0.15 * total - avail; if (target < 0) target = 0;
		} else target = 0;
	}
	if (target <= 0) { puts("enough free space; nothing to do (use --free SIZE to force a target)"); return 0; }

	sz = calloc(n_items + 1, sizeof *sz); ls = calloc(n_items + 1, sizeof *ls);
	pr = calloc(n_items + 1, sizeof *pr); chosen = calloc(n_items + 1, 1);
	for (i = 0; i < n_items; i++) { sz[i] = items[i].size; ls[i] = items[i].loss; pr[i] = items[i].prot; }
	if (!plan(sz, ls, pr, n_items, target, chosen, &freed)) {
		char h[32]; human(target, h);
		printf("cannot free %s from rebuildable items\n", h); return 1;
	}
	{
		double tot = 0; char h[32];
		for (i = 0; i < n_items; i++) if (chosen[i]) {
			human(items[i].size, h);
			printf("%8s  loss %-8.3g %s\n", h, items[i].loss, items[i].path);
			tot += items[i].loss;
		}
		human(freed, h); printf("frees %s, expected loss %.3g\n", h, tot);
	}
	if (apply) {
		int c;
		fputs("delete these? [y/N] ", stdout); fflush(stdout);
		c = getchar();
		if (c != 'y' && c != 'Y') { puts("aborted"); return 0; }
		for (i = 0; i < n_items; i++) if (chosen[i]) nftw(items[i].path, rm_cb, 32, FTW_DEPTH | FTW_PHYS);
		puts("done");
	}
	return 0;
}
