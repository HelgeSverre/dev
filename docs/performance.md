# Performance baseline

Measured 2026-08-02 with `just bench` from the implementation through
`214a2c1`, including the capability registry and the refreshed dependency
graph.

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
| exact remembered hit | 101 | 0.130 ms | 0.178 ms | 100 ms |
| structural scan, 10k files | 21 | 45.101 ms | 46.863 ms | 500 ms |
| chaos-1 conventional target scan | 21 | 46.272 ms | 47.843 ms | 750 ms |
| chaos-2 broad scan, capped at 5k | 21 | 45.943 ms | 48.932 ms | 1,500 ms |
| hinted CLI, chaos 1, 10k files | 11 | 240.346 ms | 259.027 ms | 500 ms |
| hinted CLI, chaos 2, 10k files | 11 | 317.874 ms | 382.025 ms | 750 ms |
| query, 10 candidates | 101 | 0.088 ms | 0.096 ms | 5 ms |
| query, 100 candidates | 101 | 0.908 ms | 0.952 ms | 20 ms |
| query, 1,000 candidates | 31 | 9.502 ms | 11.800 ms | 150 ms |
| query, 10,000 candidates | 31 | 96.910 ms | 105.541 ms | 1,500 ms |
| minimal process startup | 31 | 2.365 ms | 2.755 ms | 250 ms |

The three product p50 targets pass on this machine: remembered lookup is below
30 ms, structural discovery is below 120 ms, and broad hinted discovery is
below 250 ms. The pre-registry control run measured 146.771 ms p95 for the
structural scan and 788.463 ms p95 for the chaos-1 hinted CLI path; the final
implementation measures 46.863 ms and 259.027 ms respectively. No benchmark
ceiling regressed. Clipboard startup is not measured because this build has no
clipboard feature; the minimal build has no desktop dependency.
