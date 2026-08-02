# Performance baseline

Measured 2026-08-02 with `just bench` from the `ea402fd` implementation plus
the benchmark harness committed with this report.

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
| exact remembered hit | 101 | 0.093 ms | 0.129 ms | 100 ms |
| structural scan, 10k files | 21 | 42.604 ms | 43.971 ms | 500 ms |
| chaos-1 conventional target scan | 21 | 44.332 ms | 45.766 ms | 750 ms |
| chaos-2 broad scan, capped at 5k | 21 | 62.208 ms | 64.942 ms | 1,500 ms |
| query, 10 candidates | 101 | 0.170 ms | 0.194 ms | 5 ms |
| query, 100 candidates | 101 | 1.846 ms | 1.933 ms | 20 ms |
| query, 1,000 candidates | 31 | 18.600 ms | 18.812 ms | 150 ms |
| query, 10,000 candidates | 31 | 196.271 ms | 200.947 ms | 1,500 ms |
| minimal process startup | 31 | 2.130 ms | 2.357 ms | 250 ms |

The three product p50 targets pass on this machine: remembered lookup is below
30 ms, structural discovery is below 120 ms, and broad hinted discovery is
below 250 ms. Clipboard startup is not measured because this build has no
clipboard feature; the minimal build has no desktop dependency.
