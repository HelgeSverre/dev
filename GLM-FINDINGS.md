# Architecture review — `dev` (dev-launcher)

**Date:** 2026-08-03
**Reviewer:** GLM-5.2 (architecture-fit-contract-review skill)
**Scope:** whole-codebase review of src/ (75 files, ~13.4k LOC), tests/ (5 files, ~4.2k LOC), benches/, docs/. No diff — a snapshot review. No changes made.

**Overall:** This is a remarkably disciplined codebase. The spec is normative and the implementation tracks it closely. The reward-hacking invariants from `AGENTS.md` are *enforced by code*: `tests/detector_contract_test.rs` parses every detector with `syn` and rejects `Command::new`, `process::exit`, `fs::write/…`, `net::*`, `tokio::process`, etc., and insists every `env::var` key is registered in `cache_environment()`. Zero `todo!`/`unimplemented!` anywhere; every detector implements a real positive `detect()` path (four intentionally-empty detectors contribute via `TargetRunner`, which is wired in `registry.rs`). Tests are layered honestly: unit, structural-golden (64-fixture corpus, byte-identical re-run), query corpus, CLI end-to-end against the real binary, Unix exec (signal/TTY/non-UTF-8), and Windows (Job Object, Ctrl-C, exact argv). The findings below are genuine but small-bore.

---

## [STRUCTURAL] Circular module dependency between `registry` and `detect::*`

**Where:** `src/registry.rs:9-18` (imports all detector structs); `src/detect/builder.rs:11` and all 29 detector files (import `crate::registry::{…}` IDs).
**Question:** Q2

**What's wrong:** `registry` depends on the concrete detector implementations to build `&'static dyn Detector` references, while every detector — and the `CandidateBuilder` — depend back on `registry` for the `DetectorId`/`CandidateSourceId`/`ToolId` newtypes and the `*_SOURCE`/`*_TOOL` consts. This is a textbook registry-pattern cycle: the ID constants and the dispatch table live in the same module, so detectors cannot reference their own IDs without importing the table that imports them.

**Why this severity:** The skill's rubric maps module cycles to STRUCTURAL. It's contained (intra-crate, Rust resolves it, builds and tests clean) and the wiring is spec-mandated (§10.1 "registry-bound builder"), so impact today is low. But it blocks the spec's framing of the registry as a cleanly separable capability projection: neither `detect` nor `registry` can be understood, tested, or reused in isolation, and any future move to split detectors behind a feature flag or into a separate crate would require breaking this first.

**Recommended fix:** Extract the ID newtypes (`DetectorId`, `CandidateSourceId`, `ToolId`) and the `pub const` declarations into a dependency-free `src/ids.rs` (or `registry::ids`). Then `detect::*` depends only on `ids`, and `registry` depends on `detect::*` — cycle gone, no behavior change. The `DetectorRegistration`/accessors stay in `registry.rs`.

---

## [CONTRACT] `Resolution` state machine is mutated from outside `resolve`

**Where:** `src/main.rs:243-293` (`apply_remembered_choice`); `src/resolve.rs` defines `ResolutionStatus`/`ResolutionReason`.
**Question:** Q1, Q3

**What's wrong:** Three sites own `Resolution` mutation: `resolve::resolve` (the unhinted/hinted algorithms), `main::apply_remembered_choice` (sets `status`, `reason`, `selected`, and *pushes* an unavailable remembered candidate into `resolution.candidates`), and the fast-path in `main::run_resolution` that short-circuits before `resolve` runs at all. The `RememberedCommandChanged`/`RememberedActionMissing` reasons are declared in `resolve.rs` but assigned in `main.rs`, so the resolution state machine's transitions are split across module boundaries and the resolver's invariants (e.g. "selected index points into candidates, candidates sorted by §9.6/§11.8") must be re-established by hand at each external mutation site.

**Why this severity:** The binary boundary legitimately owns pipeline orchestration, but the *protocol* for converting a cache lookup into a `Resolution` is a contract that consumers of `--json`/`--why` rely on. Today only one caller exists; a second caller (a future library user, or a test that wants to assert remembered→resolved transitions) would have to re-implement this mutation logic.

**Recommended fix:** Give `resolve` (or a small `cache_integration` module) a `fn apply remembered choice` constructor that takes a `CacheLookup` and a `&Resolution` and returns the adjusted `Resolution` (or a `RememberedOutcome` enum), so all `Resolution` field writes live behind one module's API. `main.rs` then dispatches on the returned enum.

---

## [CONTRACT] `Detector::detect` signature drifts from the spec and forces per-detector allocation

**Where:** `src/detect/mod.rs:95` (`fn detect(&self, context: &ScanCtx<'_>) -> Detection;`); spec §6.1 line 389 shows `fn detect(&self, ctx: &ScanCtx, output: &mut Detection)`.
**Question:** Q3, Q4

**What's wrong:** The spec's illustrative signature takes `&mut Detection` (zero-alloc accumulation into a shared aggregator, friendly to future parallel dispatch); the implementation returns `Detection` by value, so every one of 28 detectors allocates a `Vec<Candidate>` + `Vec<Diagnostic>` that `detect_all` immediately moves and concatenates. The divergence isn't normative-MUST (the spec block is a ` ```rust ` illustration), but it's the kind of signature drift that quietly forecloses an explicit design goal (§3.1 perf targets; §8.5 "parallel detector dispatch … use only when benchmarks demonstrate a benefit").

**Why this severity:** No consumer breaks today and the cost is ~28 small allocs per scan. But it's a contract that any future contributor reading the spec will assume matches the code; the by-value form also makes a later switch to `&mut Detection` (for parallel/streaming dispatch) a breaking change to every detector.

**Recommended fix:** Either adopt the spec's `&mut Detection` signature (mechanical: change the trait + 28 `emit`-style bodies) or update spec §6.1 to record the deliberate change and the rationale. Don't leave them silently different.

---

## [FRICTION] `node.rs` is trending toward a god-module

**Where:** `src/detect/node.rs` — 1441 lines, the largest single detector.
**Question:** Q1, Q6

**What's wrong:** One module owns: package-manager inference (npm/pnpm/yarn/bun from `packageManager`/lockfiles), workspace-member resolution with per-manager selector forms, four framework detectors (Vite/Next/SvelteKit + Bun-native), `bin` entries, conventional entry files (`server.js`/`app.js`/`index.js`), and `NodeTestBinder`. Each is a distinct reason to change; adding a framework (Astro, Remix, Nuxt) extends this one file further.

**Why this severity:** It works today and the registry's per-source `metadata_priority`/`default_tags` already make framework candidates first-class. But §10.4 splits Vite and Next into their own subsections precisely because they're separate concerns, and the Goals section frames framework breadth as a growth axis. Shotgun-surgery risk is currently low (one file), but the file's cohesion is declining.

**Recommended fix:** Split framework-specific logic into `detect/node_vite.rs`, `node_next.rs`, `node_sveltekit.rs` (or a `node/frameworks/` submodule) that share a small `node::shared` helper for manager/package resolution. The `DetectorId`/source registration stays one entry in `registry.rs`; only the implementation is decomposed.

---

## [FRICTION] `decisive_match` in `main.rs` reimplements query-term ranking

**Where:** `src/main.rs:344-352`; ordering logic lives in `src/query/rank.rs` and `src/query/matcher.rs`.
**Question:** Q3, Q6

**What's wrong:** The preamble's "decisive match" is selected by a bespoke `max_by` over `class==Identity` then `class`, `quality_millis`, `points`. That ordering is a subset of `query::rank::compare_hinted` / the matcher's class precedence. Two orderings for the same concept will drift if one is tuned.

**Why this severity:** Display-only, small, and self-contained. But it's the kind of tautological-adjacent logic `AGENTS.md` flags — a contributor tuning the matcher's class weights would not know to also update the preamble selector.

**Recommended fix:** Expose a `query::rank::decisive_term(&QueryMatch) -> Option<&TermMatch>` (or reuse `compare_hinted` over the terms) and call it from `main.rs`.

---

## [FRICTION] `--list` renderer lives in `ui::why.rs`; `tools()` dedups first-wins by id

**Where:** `src/ui/why.rs` hosts both `render` (why) and `list` (--list); `src/registry.rs:1352-1366` `tools()` deduplicates with `BTreeMap::entry(id).or_insert(*tool)`.
**Question:** Q3, Q4

**What's wrong:** (a) `why.rs` is named for one informational mode but also owns the other (`--list`); a new contributor looking for the `--list` renderer won't find it by name. (b) `tools()` collapses shared `ToolId`s by *first* registration, while spec §10.1 states conflicting declarations are "programmer errors and MUST fail registry validation tests" and §10.1 "Registration order MUST NOT influence output." Today the `validate()` test catches real conflicts, but `tools()` itself silently resolves them order-dependently, so doctor rows would be wrong without the test gate.

**Why this severity:** Naming friction (a) and a latent order-dependence (b) that the contract test currently masks. Both are cheap.

**Recommended fix:** (a) Rename to `ui::summary.rs` or split `list` into its own `ui::list.rs`. (b) Have `tools()` assert identical registrations on collision (return the shared one only if equal; emit a registry-validation error otherwise) so the function is order-independent by construction, not by test.

---

## Summary table

| # | Severity | Question | File(s) | Finding |
|---|----------|----------|---------|---------|
| 1 | STRUCTURAL | Q2 | `registry.rs`, `detect/builder.rs`, 29 detectors | Circular `registry`↔`detect` module dependency |
| 2 | CONTRACT | Q1,Q3 | `main.rs:243`, `resolve.rs` | `Resolution` state mutated from outside the resolver |
| 3 | CONTRACT | Q3,Q4 | `detect/mod.rs:95`, spec §6.1 | `Detector::detect` sig returns `Detection` by value, diverges from spec |
| 4 | FRICTION | Q1,Q6 | `detect/node.rs` (1441 LOC) | God-module bundling manager+workspace+4 frameworks+test binder |
| 5 | FRICTION | Q3,Q6 | `main.rs:344` | `decisive_match` duplicates `query::rank` ordering |
| 6 | FRICTION | Q3,Q4 | `ui/why.rs`, `registry.rs:1352` | `--list` misplaced in `why.rs`; `tools()` first-wins dedup is order-dependent |

## Verdict

Per the strict skill criteria, the registry↔detect cycle is a STRUCTURAL finding → **Needs redesign**. In practice it is contained, low-impact, and partly spec-mandated, so this lands between "Needs redesign" and "Acceptable with concerns": the one STRUCTURAL item is worth fixing (a mechanical ID-module extraction with zero behavior change) but is not release-blocking given the contract test enforces the safety boundary. The CONTRACT items (2, 3) are the ones worth gating on before this codebase is treated as a library or before parallel detector dispatch is attempted. The FRICTION items (4–6) are honest-quality polish, not safety or correctness gates.

No changes were made.