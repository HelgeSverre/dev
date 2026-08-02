# Performance baseline

Measured 2026-08-02 with `just bench` from the `bc55452` baseline plus the
benchmark harness committed with this report.

- macOS 15.6 (24G84), arm64
- Apple M2 Max, 32 GiB memory
- local APFS workspace volume
- rustc/cargo 1.97.0
- release profile with thin LTO

The scan corpus is generated deterministically before timing. It contains
10,000 ordinary non-ignored files plus 250 conventional test targets. “Cold”
means a fresh `FileIndex` and no application cache; the benchmark does not purge
the operating system page cache. Each reported sample measures the complete
named operation. p95 ceilings are deliberately wider than the product targets
so CI catches gross regressions without treating normal machine variance as a
failure.

| Benchmark | Samples | p50 | p95 | p95 ceiling |
|---|---:|---:|---:|---:|
| exact remembered hit | 101 | 0.090 ms | 0.106 ms | 100 ms |
| structural scan, 10k files | 21 | 42.838 ms | 46.787 ms | 500 ms |
| chaos-1 conventional target scan | 21 | 44.502 ms | 47.107 ms | 750 ms |
| chaos-2 broad scan, capped at 5k | 21 | 64.291 ms | 66.325 ms | 1,500 ms |
| query, 10 candidates | 101 | 0.167 ms | 0.198 ms | 5 ms |
| query, 100 candidates | 101 | 1.819 ms | 1.959 ms | 20 ms |
| query, 1,000 candidates | 31 | 19.189 ms | 19.513 ms | 150 ms |
| query, 10,000 candidates | 31 | 185.347 ms | 189.713 ms | 1,500 ms |
| minimal process startup | 31 | 2.108 ms | 2.345 ms | 250 ms |

The three product p50 targets pass on this machine: remembered lookup is below
30 ms, structural discovery is below 120 ms, and broad hinted discovery is
below 250 ms. Clipboard startup is not measured because this build has no
clipboard feature; the minimal build has no desktop dependency.
