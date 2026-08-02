# `dev` — Architecture & Implementation Specification

**Status:** normative; revision 4 migration pending
**Revision:** 4 — makes detector capabilities the single source of truth
**Language:** Rust 2021  
**Audience:** implementing agent and maintainer

This document is normative. **MUST** is a hard requirement, **SHOULD** is the
default unless a measured or documented reason justifies a different choice,
and **MAY** is optional.

---

## 1. Summary

`dev` is a zero-setup command launcher for software projects. Point it at a
directory or file, state an intent, optionally add whatever words you remember,
and it discovers the commands the project already knows how to run.

```console
dev run
dev build ./apps/web
dev test ./tests/Feature entitlement participant
dev run laravel queue
dev run wibblewbale
```

The last command may match an npm script named `wibble-wabble`, a Cargo binary,
a Compose service, a Make target, or a loose executable file. `dev` ranks only
commands constructed by deterministic detectors; fuzzy matching chooses among
those commands and never invents shell syntax.

When one result is sufficiently clear, `dev` prints the exact command and
replaces itself with it. When the result is ambiguous or weak, it opens an
fzf-style picker containing the command, context, and evidence. Explicitly
remembered choices are cached per target, intent, query, and project shape.

The product is best understood as:

> An IDE Run Configuration system plus fuzzy retrieval, available everywhere
> from one fast CLI.

---

## 2. Product invariants

These invariants outrank individual scoring constants and detector details.

1. **Discover, do not define.** `dev` invokes tasks already represented by
   manifests, conventional files, or explicit runnable targets. It is not a
   task-definition language.
2. **Known commands only.** Fuzzy matching may select or combine a known runner
   template with a known target. It MUST NOT generate arbitrary shell text.
3. **No inserted installation.** `dev` MUST NOT add an install, restore,
   toolchain bootstrap, or retry step before or after the selected command. In
   particular, `npx` must never be allowed to download a missing runner. A
   selected native build command may retain its documented dependency-resolution
   behavior; that behavior belongs to the exact command shown to the user.
4. **Hints must matter.** If the user supplies meaningful hints and none match,
   `dev` MUST NOT silently auto-run the unhinted default.
5. **Ambiguity is interactive.** A close or weak decision opens the picker on a
   TTY and returns a structured ambiguity outside a TTY.
6. **Exact execution semantics.** Child stdin, stdout, stderr, terminal mode,
   signals, and exit status belong to the real command.
7. **Explain every decision.** Structural and query evidence are retained and
   renderable through `--why` and `--json`.
8. **Deterministic under identical inputs.** Filesystem iteration order, hash
   map order, detector registration order, and thread scheduling MUST NOT alter
   the selected candidate.
9. **Bounded discovery.** Normal discovery is shallow. Broader fuzzy discovery
   is explicit, capped, and surfaces truncation.
10. **No network during discovery.** Detectors parse local data only.

---

## 3. Goals and non-goals

### 3.1 Goals

- A fresh clone usually works without setup.
- Common invocations are shorter than remembering ecosystem-specific syntax.
- Word-soup queries are typo-tolerant while remaining inspectable.
- Cached resolution has a target p50 below 30 ms on a local SSD.
- Cold structural discovery has a target p50 below 120 ms on a representative
  repository containing 10,000 non-ignored files.
- Broad hinted discovery has a target p50 below 250 ms on the same corpus.
- Unix uses process replacement whenever no post-run work is required.
- CI, agents, editors, and shell integrations receive stable JSON and exit
  semantics.

Performance targets are release-build benchmarks, not correctness rules. The
benchmark machine and corpus MUST be recorded with results.

### 3.2 Non-goals

- Installing or selecting language/toolchain versions.
- Replacing Cargo, npm, Composer, Make, or other native tools.
- Defining new project tasks in a `dev`-specific project file.
- Orchestrating several selected candidates concurrently in v1.
- Inspecting Gradle/Bazel task graphs or arbitrary build-language code.
- Daemons, telemetry, remote execution, or network-client behavior implemented
  by `dev` itself.
- Using an LLM for command selection.

---

## 4. Command name

The primary executable is `dev`. Distribution naming and command collisions
MUST be checked before publication; the architecture does not depend on the
package name.

The build MAY also emit a `devx` escape-hatch binary with the same entrypoint.
If an installer uses a hardlink, it must degrade to copying on filesystems that
do not support hardlinks.

Before execution, recursion protection MUST compare the current executable and
the resolved candidate executable. On Unix, compare device and inode where
available; otherwise compare canonical paths. If they identify the same file,
abort with a recursion error.

---

## 5. CLI contract

### 5.1 Grammar

```text
dev <intent> [target] [hint ...] [-- passthrough ...]

intent := run | build | test
```

The intent is required in v1. `dev` without an intent prints help and exits 2.
An optional future shorthand may make `dev` equivalent to `dev run`, but that
must be a deliberate compatibility decision.

Examples:

```console
dev run
dev run ./apps/web
dev run ./apps/web vite frontend
dev test ./tests/Feature laravel entitlement participant
dev run wibblewbale
dev build rust release -- --release
dev test --at tests/Feature/AuthTest.php auth -- --stop-on-failure
```

### 5.2 Positional disambiguation

Ambient filesystem contents MUST NOT change whether a bare word is treated as
a hint. Therefore, a positional token is a target only when it is syntactically
path-like:

- `.`, `..`, an absolute path, or a token beginning with `./` or `../`;
- a token containing `/` or `\`;
- a target supplied explicitly through `--at <path>`.

All other positionals are hints, even if a same-named file or directory exists.
To target a bare filename, write `./server.php` or `--at server.php`.

Only one target is accepted. A syntactically path-like target that does not
exist is a usage error; it MUST NOT degrade into a hint.

This makes `dev run test` mean the same thing whether or not `./test/` exists.

### 5.3 Flags

```text
-C, --at <path>       Explicit target path
-w, --why            Render candidates, ranking, evidence, and decision; do not run
-l, --list           Terse one-line candidate list; do not run
-n, --dry-run        Resolve normally, then print the selected command; do not run
-p, --pick           Force the picker
-f, --forget         Forget the remembered choice for this target/intent/query
    --no-cache        Neither read nor write remembered choices
    --chaos <0..2>    Control fuzzy discovery breadth; default 1 when hints exist
    --depth <n>       Override structural scan depth
    --json            Emit the versioned JSON result; never run or open a picker
-q, --quiet           Suppress the execution preamble
-v, --verbose         Emit scan/detector diagnostics to stderr
    --color <mode>    auto | always | never
    --version
-h, --help

dev cache list
dev cache clear
dev doctor
```

Unknown flags before `--` are usage errors. Every argument after `--` is passed
as an opaque OS string to the selected command using its declared passthrough
style.

### 5.4 Chaos levels

Chaos controls recall, never command-generation freedom or process safety.

| Level | Discovery and matching behavior |
|---|---|
| `0` | Declared candidates only; exact segment, exact compact-name, prefix, and acronym matching; no synthetic candidates |
| `1` | Default; typo-tolerant matching, discovery-tier actions, targeted search of conventional runnable directories, and synthetic targets within those directories |
| `2` | Broad ignored-file walk up to the hard cap; synthetic candidates allowed; weaker matches may appear, but auto-selection gates do not weaken |

With no hints, chaos defaults to `0` and setting it has no effect unless
`--pick` is also present.

### 5.5 Informational modes

- `--why` and `--list` never execute and return 0 after a successful scan,
  including when the decision is ambiguous.
- `--dry-run` performs normal resolution. If a picker is needed on a TTY, the
  picker selects what to print. Without a TTY it returns ambiguity exit code 5.
- `--json` never opens the picker and writes JSON only to stdout. Its exit code
  reflects the resolution: 0 resolved, 4 none, 5 ambiguous or hint-no-match.
- Human diagnostics and the execution preamble go to stderr.

---

## 6. Core data model

Use `OsString` for executable arguments and environment values so Unix paths
and arguments are not forced through UTF-8. Search/display projections may use
lossy Unicode, but execution MUST preserve original bytes.

```rust
pub type Points = i32;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize)]
pub enum Intent { Run, Build, Test }

#[derive(Clone, Debug)]
pub enum Target { Directory(PathBuf), File(PathBuf) }

#[derive(Clone, Debug)]
pub struct Invocation {
    pub intent: Intent,
    pub target: Target,
    pub hints: Vec<String>,
    pub passthrough: Vec<OsString>,
    pub chaos: u8,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize)]
pub enum SelectionPolicy {
    /// May win an unhinted or hinted automatic resolution.
    Automatic,
    /// Hidden from unhinted automatic resolution; may auto-run only after a
    /// direct identity match from a hint.
    ExplicitHint,
    /// Always requires picker/explicit candidate selection.
    Confirm,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize)]
pub enum CandidateOrigin { Declared, Conventional, Synthetic }

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize)]
pub enum CommandLayer {
    /// A repository-level facade such as a Just recipe or Make target.
    ProjectFacade,
    /// A declared ecosystem task such as an npm or Composer script.
    EcosystemTask,
    /// A native ecosystem default inferred from project structure.
    ToolDefault,
    /// A directly selected binary, source file, test, or other target.
    DirectTarget,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize)]
pub enum Lifecycle { Finite, LongRunning, MultiProcess }

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub struct DetectorId(&'static str);

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub struct CandidateSourceId(&'static str);

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub struct ToolId(&'static str);

#[derive(Clone, Debug, Serialize)]
pub enum Availability {
    Available { resolved_program: PathBuf },
    MissingProgram { program: OsString },
    UnsupportedHost { reason: String },
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize)]
pub enum PassthroughStyle { Append, DoubleDash, NpmRun, Custom }

#[derive(Clone, Debug, Serialize)]
pub enum EvidenceKind {
    Manifest,
    Convention,
    Proximity,
    Availability,
    Rule,
}

#[derive(Clone, Debug, Serialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub reason: String,
    pub points: Points,
    pub source: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SearchDocument {
    /// Script, binary, target, service, or task names.
    pub identities: Vec<String>,
    /// Files or directories directly targeted by the command.
    pub target_paths: Vec<PathBuf>,
    /// Package, workspace member, and service scopes.
    pub scopes: Vec<String>,
    /// Ecosystem and semantic tags such as `laravel`, `rust`, `worker`.
    pub tags: Vec<String>,
    /// Human-facing text. Weakest search surface.
    pub text: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: CandidateId,
    /// Example: `node:packages/web:script:dev`.
    pub action_key: String,
    /// Implementation that emitted the candidate, e.g. `node`.
    pub detector: DetectorId,
    /// User-facing specialization, e.g. `node`, `vite`, or `next`.
    pub source: CandidateSourceId,
    pub intent: Intent,
    pub action_name: String,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Project/member scope used for proximity and facade dominance. This may
    /// differ from `cwd` for workspace commands.
    pub scope_root: PathBuf,
    pub env: BTreeMap<OsString, OsString>,
    pub passthrough: PassthroughStyle,
    pub lifecycle: Lifecycle,
    pub origin: CandidateOrigin,
    pub layer: CommandLayer,
    pub selection: SelectionPolicy,
    pub availability: Availability,
    pub base_points: Points,
    pub structural_points: Points,
    pub evidence: Vec<Evidence>,
    pub search: SearchDocument,
    pub label: String,
    pub description: String,
}
```

`CandidateId` MUST hash the normalized executable command identity after
deduplication:

```text
intent + program + args + cwd + env + passthrough style
```

It MUST NOT depend on detector name, evidence ordering, labels, localized
display strings, or the current structural score. This lets two detectors that
discover the same executable command converge on one candidate identity.

### 6.1 Detector result and diagnostics

Detector failures must remain visible without preventing independent detectors
from succeeding.

```rust
pub struct Detection {
    pub candidates: Vec<Candidate>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct Diagnostic {
    pub detector: DetectorId,
    pub severity: Severity,
    pub message: String,
    pub source: Option<PathBuf>,
}

pub enum Severity { Info, Warning, Error }

pub trait Detector: Send + Sync {
    fn detect(&self, ctx: &ScanCtx, output: &mut Detection);
}
```

Detectors MUST NOT spawn subprocesses, write files, access the network, or
source executable configuration. They may ask a shared `PathResolver` to inspect
`PATH` without invoking `--version`.

`ScanCtx` also exposes a bounded `PathProbe` for exact known paths such as
`vendor/bin/pest`; this may stat/read explicitly requested files but may not
walk an ignored dependency directory.

Detector names, synonyms, tools, markers, target hooks, and metadata precedence
belong to the registration described in section 10.1. They MUST NOT be repeated
as methods or match tables elsewhere in the codebase.

---

## 7. Pipeline

```text
argv
  -> parse intent, target, hints, passthrough
  -> resolve logical anchor and canonical cache identity
  -> resolve package root, workspace root, and scan root
  -> validate fast cache snapshot
       exact remembered hit -> preamble -> exec
       miss/stale           -> continue
  -> build structural file index
  -> run detectors and collect diagnostics
  -> deduplicate and compute structural points
  -> if hints: build/query wider target index as chaos level requires
  -> if hints: add discovery/synthetic candidates and compute query matches
  -> resolve
       none             -> actionable error
       clear winner     -> optional remember -> preamble -> exec
       weak/ambiguous   -> picker -> optional remember -> preamble -> exec
  -> propagate child result
```

The TUI MUST restore the terminal before execution. `--json`, `--why`, `--list`,
and `--dry-run` branch before execution.

---

## 8. Paths, roots, and scanning

### 8.1 Logical and physical paths

Maintain two path identities:

- **Logical path:** absolute path formed from the user's current directory
  without resolving symlinks. Use it for display, `cwd`, and execution.
- **Physical identity:** canonical path when canonicalization succeeds. Use it
  only for cache keys and recursion checks.

Do not silently replace the logical working directory with its canonical form;
some build tools and scripts observe `$PWD` or depend on symlink layout.

### 8.2 Root resolution

Starting at the anchor directory, walk ancestors to the nearest repository
boundary: `.git` directory or file, filesystem boundary, home directory, or
root. Root discovery itself is an upward stat/read operation and does not
depend on the later file index.

Resolve three distinct concepts:

1. **Package root:** nearest ancestor containing an applicable package marker,
   such as `package.json`, `Cargo.toml`, `composer.json`, `go.mod`, `build.zig`,
   `Package.swift`, `pubspec.yaml`, or a Justfile.
2. **Workspace root:** nearest enclosing explicit workspace marker, such as
   `pnpm-workspace.yaml`, `go.work`, Cargo `[workspace]`, or a `package.json`
   containing `workspaces`. Continue above the package root to find it.
3. **Scan root:** workspace root when present; otherwise package root;
   otherwise the repository boundary; otherwise the anchor directory.

This distinction is mandatory. Stopping at a member package's `package.json`
must not hide its enclosing pnpm workspace.

Applicable markers come from the detector registry. `Classified` markers are
interpreted through their registration's bounded `WorkspaceContributor`; root
resolution MUST NOT contain an ecosystem-name match table.

### 8.3 Scan modes

`dev` uses two bounded indexes:

**Structural index** — always built on a cache miss:

- Default maximum depth: 3 beneath the scan root.
- Always include the anchor itself and its direct children, even when the
  anchor lies near the structural depth boundary.
- Explicit workspace member manifests declared by a parsed workspace are
  indexed even when they lie beyond the nominal depth, subject to the same hard
  cap.
- Record files and directories needed for manifest, platform, and workspace
  decisions.
- Maximum 20,000 entries.

**Target index** — built only when hints or forced picking require broader
recall:

- Chaos 0: not built.
- Chaos 1: recursively scan conventional roots discovered structurally:
  `tests/`, `test/`, `spec/`, `examples/`, `scripts/`, `bin/`, `cmd/`, and
  ecosystem-specific equivalents.
- Chaos 2: recursively scan the scan root while respecting ignore rules.
- Maximum 20,000 additional entries.

Both indexes MUST respect `.gitignore`, `.ignore`, and default VCS ignores;
MUST ignore `.git`, `node_modules`, `vendor`, `target`, common build output, and
tool caches; and MUST NOT follow directory symlinks by default.

If a cap is reached, set `truncated = true`, record the skipped scope, and show
that fact in `--why`, JSON, and the picker. A truncated scan may lower
confidence but may not silently pretend to be exhaustive.

### 8.4 File index

```rust
pub struct FileIndex {
    pub structural: Vec<IndexEntry>,
    pub targets: Vec<IndexEntry>,
    pub by_name: HashMap<OsString, SmallVec<[EntryId; 2]>>,
    pub by_extension: HashMap<OsString, SmallVec<[EntryId; 8]>>,
    pub manifests: ManifestCache,
    pub truncated: Vec<Truncation>,
}

pub struct IndexEntry {
    pub relative_path: PathBuf,
    pub file_type: IndexedFileType,
    pub executable: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}
```

Manifest parsing is lazy and memoized. Parse errors become detector diagnostics.
The cache must be concurrency-safe if detectors run in parallel.
Root classification, workspace scan expansion, and normal detector parsing use
the same `DiscoveryFiles`/manifest cache so every semantic read is recorded once
for cache invalidation.

### 8.5 Determinism and concurrency

Every collected path and candidate MUST be sorted by a stable bytewise path or
candidate key before ranking. A hard-cap traversal must visit entries in a
deterministic order; a parallel walker that returns an arbitrary first 20,000
entries is not acceptable.

Parallel walking and detector dispatch are implementation choices, not
requirements. Use them only when benchmarks demonstrate a benefit. The
resolver must never observe scheduler-dependent ordering.

---

## 9. Structural candidates and scoring

### 9.1 Candidate tiers

| Tier | Typical source | Selection policy |
|---|---|---|
| Canonical | `dev`, `start`, `build`, `test`, sole binary | `Automatic` |
| Alternative | `serve`, `watch`, example, secondary binary | `Automatic` or `ExplicitHint` |
| Discovery | arbitrary npm/Composer/Make task, executable in `scripts/` | `ExplicitHint` |
| Materially different | deliberately confirm-only host/platform action | `Confirm` |
| Synthetic | inferred runner + fuzzy-matched file | `ExplicitHint` |

Selection policy is semantic state. It MUST NOT be simulated by giving a
candidate a low score that proximity can accidentally raise above a threshold.

### 9.2 Points

Structural points are signed integers and are not clamped. A displayed number
is a rank score, never a probability.

```text
structural_points = base_points
                  + sum(unique structural evidence points)
                  + proximity points
                  + resolver rule points
```

Recommended initial constants:

```rust
const AUTO_FLOOR: Points = 30;
const CLEAR_WINNER_MARGIN: Points = 15;
const SAME_DIR_POINTS: Points = 30;
const EXACT_FILE_POINTS: Points = 40;
const MISSING_PROGRAM_POINTS: Points = -50;
```

Detector base suitability commonly ranges from 15 to 95. Scores above 100 are
valid and preserve ranking resolution.

### 9.3 Proximity

Let `d` be directory-edge distance from the anchor directory to the candidate
working directory:

```text
d = 0 -> +30
d = 1 -> +15
d = 2 ->  +8
d = 3 ->  +4
d > 3 ->  +0
```

If the anchor is a file and the command directly targets that exact file, add
40 points. This bonus applies once after deduplication.

### 9.4 Availability

Availability does not change the candidate's existence:

- Missing or host-incompatible candidates remain visible.
- They are never automatically selected.
- Explicit selection produces a clean diagnostic before attempting `exec`.

Program lookup must use the effective environment for that candidate, but MUST
NOT execute the program.

### 9.5 Deduplication

Candidates are execution-equivalent when normalized `(program, args, cwd, env,
passthrough)` are equal. When merging duplicates:

1. Keep the highest base suitability.
2. Deduplicate evidence by `(kind, reason, source)`.
3. Merge search identities, scopes, tags, and paths as sets.
4. Keep the strictest selection policy (`Confirm` > `ExplicitHint` >
   `Automatic`).
5. Prefer the most specific detector's label and description according to a
   fixed registry, e.g. Vite over Node and Artisan over Composer.
6. Recompute structural points from the merged result.

Never concatenate evidence while retaining a previously computed total.

### 9.6 Stable ordering

Unhinted candidates sort by:

1. availability;
2. automatic eligibility;
3. structural points descending;
4. anchor distance ascending;
5. action key bytewise ascending;
6. candidate ID bytewise ascending.

The final two keys exist only to make ties deterministic; they do not imply
greater confidence.

### 9.7 Declared task-layer dominance

A canonical declared task is the project's interface and may contain
setup, teardown, service orchestration, environment preparation, or several
native commands. Higher declared layers therefore dominate lower layers for the
same intent and `scope_root` during unhinted resolution. For example, a Just
recipe named `test` outranks a Composer `test` script when the Just recipe wraps
unit tests, Docker setup, integration tests, and cleanup.

The precedence is:

```text
ProjectFacade > EcosystemTask > ToolDefault
```

Dominance changes automatic eligibility, not candidate existence or execution
identity:

1. Find available `Automatic` declared-task candidates whose identity is
   canonical for the requested intent.
2. For each such candidate, demote same-scope candidates in strictly lower
   layers to `ExplicitHint` for this resolution.
3. Keep demoted candidates visible in `--why`, `--list`, JSON, and the picker.
4. A direct identity hint for a demoted candidate may still select it.
5. Two distinct candidates in the same layer remain competitors; do not choose
   between Just and Make or between npm and Composer solely by registration
   order.

Framework fallbacks and native conventions SHOULD use `ToolDefault`. Declared
npm/Composer scripts SHOULD use `EcosystemTask`. Just, Jake, Taskfile, and mise
tasks and literal Make targets SHOULD use `ProjectFacade`. Explicit files and
binaries use `DirectTarget`.

---

## 10. Detector specification

### 10.1 Registry

The registry is a static capability registry, not a plugin loader and not one
large trait with mostly empty methods. Each entry combines immutable metadata
with only the hooks that ecosystem needs:

```rust
pub struct DetectorRegistration {
    pub id: DetectorId,
    pub candidate_sources: &'static [CandidateSourceRegistration],
    pub synonyms: &'static [&'static str],
    pub markers: &'static [ProjectMarker],
    pub tools: &'static [ToolRegistration],
    pub conventional_roots: &'static [&'static str],
    pub candidate_schema: u32,
    pub detector: &'static dyn Detector,
    pub workspace: Option<&'static dyn WorkspaceContributor>,
    pub target_binders: &'static [&'static dyn TargetBinder],
    pub target_runners: &'static [&'static dyn TargetRunner],
}

pub struct CandidateSourceRegistration {
    pub id: CandidateSourceId,
    pub metadata_priority: u8,
    pub default_tags: &'static [&'static str],
}

pub struct ProjectMarker {
    pub pattern: MarkerPattern,
    pub root_role: RootRole,
}

pub enum MarkerPattern {
    Exact(&'static str),
    AsciiCaseInsensitiveBasename(&'static str),
    Extension(&'static str),
}

pub enum RootRole {
    Package,
    Workspace,
    /// A bounded parser decides whether the marker is a package, workspace, or
    /// both, as with Cargo.toml and package.json.
    Classified,
    Auxiliary,
}

pub struct ToolRegistration {
    pub id: ToolId,
    pub program: &'static str,
    pub doctor: DoctorProbe,
}

pub enum DoctorProbe {
    Command {
        args: &'static [&'static str],
        timeout: Duration,
    },
    LocalMetadata(fn(resolved_program: &Path) -> ProbeOutcome),
    PresenceOnly { reason: &'static str },
}
```

`candidate_sources` separates the implementation that found an action from the
specialized source shown to the user. For example, the Node implementation may
emit `node`, `vite`, and `next` sources, and the Dart implementation may emit
`dart` and `flutter`. Metadata precedence used during deduplication comes from
the source registration; it MUST NOT live in a string match table.

The registry is the single source of truth for all of these consumers:

- detector dispatch;
- package/workspace marker lookup during upward root resolution;
- marker reporting in no-candidate diagnostics;
- cache shape inputs and cached detector/source restoration;
- bounded workspace-member scan expansion;
- chaos-1 conventional target roots;
- target binders and standalone target runners;
- tool availability and `dev doctor` probes.

Adding a detector MUST NOT require adding its name or marker to another module.
Infrastructure may keep derived, sorted indexes for performance, but those
indexes MUST be built from the registry. Exact duplicate declarations are
deduplicated. Conflicting declarations for the same detector, source, or tool
ID are programmer errors and MUST fail registry validation tests.

Markers read through the bounded manifest reader become semantic cache inputs
automatically. Exact marker paths are watched whether present or absent. For
case-insensitive or extension patterns, the shape snapshot records a sorted
projection of matching directory entries so creating a new `Justfile` or
`*.csproj` invalidates a remembered choice. The cache stores a deterministic
fingerprint of the sorted candidate-relevant registration metadata, including
detector/source/tool IDs, programs, markers, priorities, conventional roots,
and `candidate_schema`, instead of a manually maintained global detector
number. Hook behavior that is not represented in static metadata requires a
`candidate_schema` bump. A candidate source is restored only through a current
registry lookup.

Workspace support is an optional pure-data hook:

```rust
pub trait WorkspaceContributor: Send + Sync {
    fn classify_root(
        &self,
        marker: &Path,
        files: &DiscoveryFiles,
    ) -> RootClassification;

    fn scan_contribution(
        &self,
        root: &Path,
        files: &DiscoveryFiles,
    ) -> ScanContribution;
}
```

`DiscoveryFiles` is bounded, memoized, read-only, and shared with the manifest
cache. `ScanContribution` contains sorted include/exclude patterns and semantic
input paths. This moves Cargo member globs, Node/pnpm workspaces, and `go.work`
members behind the registrations that understand them while keeping root and
scan orchestration ecosystem-agnostic.

The initial registry is static. Registration order MUST NOT influence output.

| Registration | Candidate sources | Primary markers | Intents |
|---|---|---|---|
| `node` | `node`, `vite`, `next` | `package.json`, lockfiles | run, build, test |
| `cargo` | `cargo` | `Cargo.toml` | run, build, test |
| `composer` | `composer` | `composer.json` | run, build, test |
| `artisan` | `artisan` | `artisan`, Laravel package evidence | run, test |
| `php-file` | `php-file` | explicit or hinted `*.php` | run |
| `go` | `go` | `go.mod`, `go.work`, `package main` | run, build, test |
| `zig` | `zig` | `build.zig`, explicit `*.zig` | run, build, test |
| `swift` | `swift` | `Package.swift` | run, build, test |
| `dart` | `dart`, `flutter` | `pubspec.yaml` | run, build, test |
| `python-file` | `python-file` | explicit or hinted `*.py` | run |
| `just` | `just` | case-insensitive `justfile`, `.justfile` | run, build, test |
| `jake` | `jake` | `Jakefile` | run, build, test |
| `taskfile` | `taskfile` | standard Taskfile YAML names | run, build, test |
| `mise` | `mise` | `mise.toml`, `.mise.toml`, environment/local variants | run, build, test |
| `sema` | `sema` | `sema.toml`, `*.sema` | run, build, test |
| `make` | `make` | Makefile variants | run, build, test |
| `docker` | `docker` | Compose files, `Dockerfile` | run, build |
| `shell` | `shell` | shebangs, executable files | run |

Full Python project support, CMake, Bazel, Nix, and plugin detectors are
deferred. Gradle, Maven, and .NET follow the static expansion contract in
section 10.19.

### 10.2 Shared detector rules

Every emitted candidate MUST provide:

- a stable `action_key` and human action name;
- explicit program, argv, working directory, environment delta, and
  passthrough style;
- a base suitability and selection policy;
- at least one structural evidence item;
- search identities, scopes, tags, and target paths;
- lifecycle and origin;
- availability evaluated by the shared resolver.

Raw `Candidate::new` construction is private to the detection infrastructure.
Detectors emit through a registry-bound builder so required metadata and shared
policy cannot be skipped:

```rust
ctx.candidates
    .declared_task(JUST_SOURCE, intent, scope, recipe.name())
    .tool(JUST_TOOL)
    .args(["--justfile", justfile, recipe.name()])
    .cwd(project_root)
    .passthrough(PassthroughStyle::Append)
    .selection(selection)
    .base_points(points)
    .lifecycle(lifecycle)
    .evidence(manifest_evidence)
    .search(search_document)
    .emit(output);
```

The builder validates that the source and tool belong to a current
registration, attaches detector identity, default tags, and synonyms, resolves
availability, and derives the initial candidate ID. An incomplete candidate
becomes a detector diagnostic in release builds and a test failure in detector
fixtures. Direct executable paths use an explicit `program_path` builder method
instead of inventing a tool registration.

Commands must use argv arrays. Detectors MUST NOT produce `sh -c` strings.
Manifest commands that are defined by an ecosystem as shell snippets remain
inside that ecosystem's runner, e.g. `npm run dev` or `composer run-script dev`.

### 10.3 Node and package scripts

Resolve the package manager in this order and retain the reason as evidence:

1. `packageManager` in `package.json`;
2. `bun.lock` or `bun.lockb`;
3. `pnpm-lock.yaml`;
4. `yarn.lock`;
5. `package-lock.json`;
6. default `npm` with a small negative evidence adjustment.

Map declared scripts:

| Intent | Canonical names, in preference order |
|---|---|
| Run | `dev`, `start`, `serve`, `watch` |
| Build | `build`, `compile`, `bundle` |
| Test | `test`, `test:unit`, `spec`, `vitest`, `jest` |

- Canonical matches are `Automatic`, with initial base points between 95 and
  65 according to preference order.
- Every other script is emitted under Run as `ExplicitHint`, base 15, and is
  visible through hints or forced picking.
- A `bin` entry is an explicit runnable target. A `main` field alone is library
  metadata and MUST NOT be treated as proof that the package should be run.
- Conventional `server.js`, `app.js`, or `index.js` fallbacks are
  `ExplicitHint` unless directly anchored.

Package-manager command construction and argument forwarding MUST have
integration fixtures for each supported manager. Representative forms are:

```text
npm run <script>
pnpm run <script>
yarn run <script>
bun run <script>
```

For workspaces, determine both the member name and relative path. Prefer the
manager's documented member selector; use the relative path when a package is
unnamed. Never infer that all managers share the same filter flag ordering.

### 10.4 Vite and Next

Vite triggers on a Vite config or declared dependency:

- Run: declared `dev`, otherwise a local-only package-manager exec of `vite`.
- Build: declared `build`, otherwise local-only exec of `vite build`.
- Preview: discovery alternative under Run, `ExplicitHint`, with a description
  saying a prior build is required.

Next triggers on a Next config or dependency:

- Run: `next dev`/declared `dev` as canonical; `next start` as an explicit
  alternative requiring a prior build.
- Build: `next build`/declared `build`.
- Test: only a declared package test script. Next itself does not imply a test
  command.

Node and framework candidates commonly deduplicate. Framework evidence and
labels win while preserving the package-script evidence.

### 10.5 Cargo

Parse TOML; do not use regexes. Static discovery MUST cover:

- `package.default-run`;
- explicit `[[bin]]` targets;
- implicit `src/main.rs`;
- implicit `src/bin/*.rs` and `src/bin/*/main.rs`;
- explicit `[[example]]` and conventional examples;
- workspace `members`, `exclude`, and `default-members`, including manifest
  globs expanded against the file index.

Commands:

- Single package with one executable: `cargo run`, base 95.
- Multiple executables: one `cargo run --bin <name>` candidate each. The
  `default-run` target is canonical; otherwise a package-name match receives a
  preference but does not suppress ambiguity.
- Workspace member: `cargo run -p <package>` plus `--bin` when needed.
- Example: `cargo run --example <name>`, `ExplicitHint`, base 25.
- Build: `cargo build`, or `cargo build --workspace` at a virtual workspace.
- Test: `cargo test`, or `cargo test --workspace` at a virtual workspace.

A library-only crate emits no Run candidate and contributes an actionable
diagnostic suggesting Build, Test, and any examples. Do not claim a test count
unless tests were actually indexed.

The default Build command is the ecosystem default. `--release` is passed
explicitly after `--`; `dev` does not redefine “build” as “release artifact.”

### 10.6 Composer, Artisan, and PHP

Composer scripts may be strings or arrays and are emitted through:

```text
composer run-script <name>
```

Mappings:

| Intent | Names |
|---|---|
| Run | `dev`, `serve`, `start` |
| Build | `build` |
| Test | `test`, `phpunit`, `pest` |

Other scripts are `ExplicitHint`. When no test script exists, an existing
`vendor/bin/pest` or `vendor/bin/phpunit` may be emitted using its explicit
project-relative executable path. Do not globally prepend `vendor/bin` to
`PATH`.

Artisan requires an `artisan` file plus Laravel package evidence:

- Composer `dev`, when present, is the preferred Run candidate and is marked
  `MultiProcess` if it conventionally starts several services.
- `php artisan serve` is an alternative Run candidate.
- `php artisan test` is the canonical Test candidate.

A PHP file is directly runnable when explicitly anchored. Hinted loose PHP
files are synthetic/explicit-hint candidates. An executable PHP file with a
valid shebang runs directly; otherwise use the resolved PHP interpreter.

### 10.7 Go

Static discovery reads `go.mod`, `go.work`, and enough Go source to identify the
package clause; it does not invoke `go list`.

- A directory containing `package main`: `go run .` and `go build .`.
- Conventional `cmd/<name>` main packages: `go run ./cmd/<name>` and
  `go build ./cmd/<name>`.
- Test: `go test ./...` from the selected module, or a more local package when
  the anchor points inside one.
- A `.go` hint matching one file in a multi-file main package should still
  target the containing package (`go run .`), not only that file, unless static
  inspection proves the file is standalone.

Do not add `-o <name>` by default; that changes Go's native build behavior and
requires naming policy outside `dev`'s scope.

### 10.8 Zig

- `build.zig`: Run `zig build run`, Build `zig build`, Test `zig build test`.
- Explicit standalone Zig file: `zig run <file>`.
- `build.zig.zon` may be parsed for package-name evidence.

`build.zig` is executable Zig code and MUST NOT be parsed heuristically for all
steps. Surface the limitation as zero-point evidence. Passthrough placement for
`zig build run` and `zig build <step>` must be modeled separately.

### 10.9 Swift

`Package.swift` is Swift code and is not executed during discovery. Infer
targets conservatively from conventional `Sources/<Target>/main.swift` and
related layouts:

- Run: `swift run` for one executable, or `swift run <target>` for each known
  executable target.
- Build: `swift build`.
- Test: `swift test`, preferred when `Tests/` exists.

Bare Xcode projects produce no executable candidate unless scheme and
destination data are available from a supported data source. Emit a diagnostic
explaining that a valid `xcodebuild` invocation cannot be inferred from filename
conventions alone.

### 10.10 Flutter and Dart

Parse `pubspec.yaml`. A Flutter SDK dependency distinguishes Flutter from plain
Dart.

- Flutter Run: `flutter run`, LongRunning.
- Flutter Build: one `ExplicitHint` candidate per generated platform directory.
  Host-incompatible targets remain visible as `UnsupportedHost` and cannot
  auto-run.
- Flutter Test: `flutter test`.
- Dart Run: `dart run`, or `dart run <file>` for an anchored file.
- Dart Test: `dart test`.

The detector does not enumerate connected devices because detection may not
spawn commands. The description must state that Flutter may prompt.

### 10.11 Python file support

V1 supports explicitly anchored and strongly hinted standalone Python files
without claiming full Python-project understanding:

- Prefer `python` when it resolves inside an active virtual environment.
- Otherwise prefer `python3`, then `python` when available.
- If a project is demonstrably managed by an installed, local-only runner such
  as `uv`, a detector may emit its explicit invocation.
- Never use a command that can install a missing runner.

`pyproject.toml` entry points, pytest, uv/Poetry/PDM environments, editable
installs, and monorepo behavior require a dedicated post-v1 design.

### 10.12 Sema

Recognize `sema.toml`, `sema.lock`, and Sema source files. Parse `sema.toml`
as bounded TOML. A `[package]` table may declare `entrypoint`; package manifests
without one use `package.sema` only when that file exists. Entrypoints must be
literal relative paths that remain inside the package.

- Run: `sema <entrypoint>`.
- Build: `sema build <entrypoint>`.
- Test: execute conventional `tests.sema` and `*.test.sema` suites as `sema
  <file>`.
- Hinted standalone sources use `sema <file>` or `sema build <file>` according
  to intent. A standalone source is a Test candidate only when its filename is
  a conventional test name, and it still executes as `sema <file>`.

There is no inferred `sema test` or doctest mode. `sema --version` is the
registered doctor probe. Package-manager commands are never inserted before
execution.

### 10.13 Just

Recognize `.justfile` and any basename equal to `justfile` under ASCII
case-insensitive comparison. A Justfile is a package marker. If several
accepted spellings occur in one directory and a deterministic native precedence
cannot be established, retain the candidates but require confirmation and emit
a diagnostic.

Discovery parses Justfiles as bounded local data. It MUST NOT invoke `just
--list`, `--summary`, `--dump`, `--json`, or any other Just command: parsing or
evaluating a Justfile can load imports and evaluate expressions. Execution uses
the exact discovered file:

```text
just --justfile <absolute-justfile> <recipe>
```

The initial conservative parser:

- emits public recipes whose minimum positional arity is zero;
- accepts defaulted parameters and zero-minimum `*args` parameters;
- omits recipes with required positional parameters or `+args`, with an info
  diagnostic explaining why;
- excludes underscore-prefixed recipes and recipes or aliases marked
  `[private]`;
- retains dependency-only recipes because they commonly express orchestration;
- uses recipe doc comments as descriptions;
- resolves non-private aliases into additional search identities on the target
  recipe instead of emitting a second command;
- reads only literal, relative imports/modules within the scan root, with a
  depth and file-count cap; unsupported dynamic or escaping paths produce a
  diagnostic rather than partial execution semantics.

Run names are `run`, `dev`, `start`, `serve`, and `watch`; Build names are
`build`, `all`, `compile`, and `bundle`; Test names are `test`, `check`, and
`verify`. Canonical recipes are `Automatic`. Other public recipes are Run
`ExplicitHint` actions. A zero-arity default recipe may also be an Automatic Run
candidate when its name is not canonically Build or Test.

Compound names containing an intent segment, such as `build-release`,
`test:integration`, or `check_fast`, are `ExplicitHint` candidates for that
intent. They do not become unhinted defaults.

Just recipes use `CommandLayer::ProjectFacade`. A canonical recipe therefore
dominates lower-level same-scope defaults under section 9.7 while leaving those
alternatives directly selectable. `just --version` is the registered doctor
probe; `just version` is a recipe invocation and MUST NOT be used as a probe.

### 10.14 Jake, Taskfile, and mise

These systems are declared project-task facades and follow the canonical task
names and dominance rules used by Just. Discovery is static and MUST NOT invoke
`jake --list`, `task --list`, `mise tasks`, or any other native listing command.
Unsupported dynamic includes invalidate that facade's partial result and emit a
diagnostic.

Jake recognizes the case-sensitive `Jakefile`, public `task` and simple recipe
headers, `@default`, `@desc`, adjacent doc comments, aliases, and bounded
literal relative `@import` directives. Dot namespaces are preserved. File
recipes, underscore-prefixed recipes, and tasks with required parameters are
not auto candidates. Execution is:

```text
jake -f <absolute-Jakefile> <task> -- <passthrough...>
```

Taskfile recognizes the standard `Taskfile.yml`, `Taskfile.yaml`, lowercase,
and `.dist` spellings with documented precedence. It parses public YAML tasks,
descriptions, aliases, `internal`, required variables, and bounded literal
includes. Included namespaces use `:`. A canonical task that declares required
variables requires confirmation. Execution is:

```text
task --taskfile <absolute-Taskfile> <task> -- <passthrough...>
```

mise recognizes `mise.toml`, `.mise.toml`, and environment/local variants. It
parses `[tasks]` entries, descriptions, aliases, hidden tasks, dependency-only
tasks, and bounded literal `[task_config].includes`. It does not evaluate tool,
environment, hook, or task scripts during discovery. Execution is:

```text
mise run <task> -- <passthrough...>
```

Their registered doctor probes are respectively `jake --version`, `task
--version`, and `mise --version`.

### 10.15 Make

Use a conservative line scanner, not a claim of fully parsing Make syntax.
Recognize literal target names; ignore pattern rules, special targets, variable
expansions, and `.PHONY` bookkeeping. Capture adjacent `##` comments as help.

- Run names: `run`, `dev`, `serve`, `start`.
- Build names: `build`, `all`.
- Test names: `test`, `check`.
- Other literal targets: `ExplicitHint`, base 15.

Make's first literal target is its default goal, but that does not establish
Run/Build/Test intent. Record default-goal evidence for display and matching;
do not make an otherwise non-canonical target Automatic. In particular, a
first `help`, `lint`, or `format` target must not become a Build facade and
demote a native build candidate under section 9.7.

Make is often a wrapper. When an execution-equivalent native candidate exists,
deduplication should prefer the native explanation; otherwise Make remains a
valid candidate rather than receiving a blanket penalty.

### 10.16 Docker and Compose

Recognize all standard Compose filename spellings:

```text
compose.yml
compose.yaml
docker-compose.yml
docker-compose.yaml
```

- Compose Run: `docker compose up`, `ExplicitHint`. It never wins unhinted
  resolution, but a direct Compose/service identity hint may select it.
- Individual services become `ExplicitHint` candidates only when the YAML can
  be parsed as data without interpolation execution.
- Dockerfile Build: `docker build .` as `ExplicitHint`.

Do not synthesize an image tag from a directory name: names may be invalid and
tagging is an unnecessary side effect.

### 10.17 Shell and executables

- An explicitly anchored executable with a shebang runs directly.
- A non-executable script with a recognized shebang runs through that
  interpreter; a `.sh` suffix alone falls back to `sh` only when no more
  specific shebang exists.
- Conventional `run.sh`, `dev.sh`, `start.sh`, `build.sh`, and `test.sh` map to
  their intents as alternatives.
- Executables in `scripts/` and `bin/` are `ExplicitHint` discovery actions.

Never source a script to inspect it.

### 10.18 Target binding and synthetic candidates

Specific ecosystem binding takes precedence over executing a matched file as a
standalone program. Detectors may register pure target binders:

```rust
pub trait TargetBinder {
    fn supports(&self, base: &Candidate, target: &IndexEntry, ctx: &ScanCtx)
        -> bool;
    fn bind(&self, base: &Candidate, target: &IndexEntry, ctx: &ScanCtx)
        -> Option<Candidate>;
}
```

Examples:

- Laravel/Pest/PHPUnit Test + `tests/Feature/AuthTest.php` becomes
  `php artisan test tests/Feature/AuthTest.php` or the detected test runner,
  not `php tests/Feature/AuthTest.php`.
- A package-manager test script + a supported JavaScript test file uses that
  script's passthrough model.
- A Go Test provider + a file inside a package becomes `go test ./<package>`.
- A Compose provider + a service identity becomes `docker compose up <service>`.

Target binders run for an explicitly anchored file even without hints. With
hints, they run only after the target index finds a sufficiently matching
target. Bound candidates retain the provider action key plus a normalized target
suffix and identify both provider and target in their evidence.

Only when no ecosystem binder claims a target may a generic runner synthesize a
standalone command.

Synthetic candidates exist only for hinted invocations at chaos level 1 or 2.
They are produced by a typed `RunnerRegistry`, not a global extension-to-string
table:

```rust
pub trait TargetRunner {
    fn supports(&self, entry: &IndexEntry, ctx: &ScanCtx) -> bool;
    fn candidate(&self, entry: &IndexEntry, ctx: &ScanCtx) -> Option<Candidate>;
}
```

The registry may support PHP, JavaScript, Python, Ruby, shell, Go, Zig, Dart,
and executable shebang targets. TypeScript is emitted only when a known local
runner is already available through project metadata; `npx tsx` without a
verified local installation is forbidden because it may access the network.

Synthetic candidates:

- have origin `Synthetic` and selection `ExplicitHint`;
- name the matched file as an identity and target path;
- include the runner decision as evidence;
- never appear in unhinted resolution;
- remain visible after the picker filter is cleared during the same invocation.

### 10.19 Expansion constraints for Gradle, Maven, and .NET

New ecosystems use the same registration, marker, workspace, candidate-builder,
and doctor APIs. They do not receive discovery-time subprocess exceptions.

| Registration | Markers and static discovery | Candidate execution | Doctor |
|---|---|---|---|
| `gradle` | `settings.gradle`, `settings.gradle.kts`, `build.gradle`, `build.gradle.kts`; conservatively parse literal project includes and plugin/task declarations | Prefer an already usable local wrapper, otherwise `gradle`; canonical tasks such as `build`, `test`, and evidenced `run` only | global `gradle --version` |
| `maven` | `pom.xml`; parse XML modules, packaging, profiles, and declared plugins without resolving them | Prefer an already usable local wrapper, otherwise `mvn`; canonical lifecycle phases and statically declared plugin goals | global `mvn --version` |
| `dotnet` | `.sln`, `.slnx`, `.slnf`, `.csproj`, `.fsproj`, `.vbproj`; parse solution/project data directly | `dotnet build`, `dotnet test`, and `dotnet run --project <path>` | `dotnet --version` |

Gradle build scripts execute during configuration, so `gradle tasks` and
`gradle help` are not discovery APIs. Maven help/plugin goals may resolve
plugins and are not discovery APIs. `dotnet sln list` is unnecessary because
solution formats can be parsed locally. Wrappers may download Gradle or Maven;
they are considered available only when the declared distribution is already
present locally. Doctor never invokes a project wrapper.

Build, test, and run commands in these ecosystems may perform their normal
dependency resolution after the user authorizes execution. `dev` MUST NOT
silently prefix an install or restore command. Missing dependency state should
produce a targeted post-failure suggestion when it can be diagnosed reliably,
not an automatic retry with network access.

---

## 11. Fuzzy hint engine

### 11.1 Design model

Hints are retrieval clues, not forwarded arguments and not a tiny natural
language. The engine treats each hint independently, then aggregates matches
across fields. It distinguishes:

- **Identity:** script, binary, target, service, or target filename;
- **Scope:** package, workspace member, directory, framework, language;
- **Context:** program, label, command words, description, and evidence.

Identity matches can identify a command. Scope matches narrow a family. Context
matches improve ordering but should rarely authorize automatic execution by
themselves.

### 11.2 Normalization

For each hint and searchable string, retain two normalized representations:

1. **Segments:** split on whitespace, `-`, `_`, `.`, `:`, `/`, `\`, and
   camel-case boundaries; lowercase each segment.
2. **Compact:** concatenate the segments.

Keep the original text for display. Do not globally strip words such as
`cargo`, `npm`, `run`, or `test`; they are useful ecosystem or intent context.

A small stable filler set (`the`, `a`, `an`, `in`, `for`, `from`, `please`,
`thing`, `whatever`) receives zero coverage weight unless it exactly matches an
identity. This supports natural word soup without allowing filler to decide a
candidate.

### 11.3 Matching strategies

Compute the best calibrated quality per hint/field using:

1. exact segment;
2. exact compact form;
3. segment prefix;
4. compact substring for hints of at least four characters;
5. acronym/initialism for hints of two to four characters and targets with at
   least two segments;
6. subsequence fuzzy matching through `nucleo` at chaos 1+;
7. Jaro-Winkler through `strsim` at chaos 1+.

Raw scores from different libraries are not directly comparable. The matcher
adapter MUST normalize them against an exact-match baseline and validate the
calibration against positive and negative corpus tests.

Initial minimum fuzzy qualities are length-sensitive:

| Hint length | Allowed fuzzy behavior |
|---|---|
| 1 | exact segment only |
| 2 | exact segment or valid acronym only |
| 3 | exact/prefix; fuzzy quality at least 0.90 |
| 4–5 | fuzzy quality at least 0.82 |
| 6+ | fuzzy quality at least 0.76 |

Chaos 2 may lower the last two thresholds by at most 0.04 for picker inclusion.
It MUST NOT lower the automatic-execution threshold.

### 11.4 Match surfaces

Recommended initial field weights:

| Field | Class | Weight |
|---|---:|---:|
| action/script/binary/service name | identity | 100 |
| target filename stem | identity | 100 |
| target path segment | identity | 90 |
| package/workspace/service scope | scope | 75 |
| candidate working-directory segment | scope | 65 |
| detector name and synonyms | scope | 60 |
| semantic tags | scope | 55 |
| command argument token | context | 40 |
| program | context | 35 |
| label | context | 30 |
| description/evidence text | context | 15 |

One hint contributes only its highest-quality weighted field match to a
candidate. Matching the same word in the label, command, and evidence must not
triple-count it.

### 11.5 Synonyms and tags

Each detector exposes static ecosystem synonyms. Individual candidates expose
semantic tags.

```text
artisan -> laravel, php, artisan
node    -> javascript, js, typescript, ts, node, npm, pnpm, yarn, bun
cargo   -> rust, rs, cargo, crate
docker  -> docker, compose, container
go      -> go, golang
swift   -> swift, spm, xcode
dart    -> dart, flutter, pub
make    -> make, makefile
```

Short ambiguous synonyms such as `go`, `js`, `ts`, and `rs` match only exact
segments, not arbitrary substrings.

### 11.6 Query match data

```rust
pub struct TermMatch {
    pub hint: String,
    pub candidate_value: String,
    pub field: SearchField,
    pub class: MatchClass, // Identity | Scope | Context
    pub strategy: MatchStrategy,
    pub quality_millis: u16, // 0..=1000
    pub points: Points,
}

pub struct QueryMatch {
    pub terms: Vec<TermMatch>,
    pub highest_class: Option<MatchClass>,
    pub meaningful_terms: u16,
    pub matched_meaningful_terms: u16,
    pub coverage_millis: u16,
    pub best_identity_quality: u16,
    pub identity_points: Points,
    pub scope_points: Points,
    pub context_points: Points,
    pub total_points: Points,
}
```

Multiple hints are additive across distinct hints. Duplicate normalized hints
count once. Unmatched hints do not subtract points, but they lower coverage.
Filler terms do not affect coverage.

### 11.7 Candidate widening

When hints are present:

1. Include `ExplicitHint` discovery candidates from all detectors.
2. Build the target index required by the chaos level.
3. Ask typed target runners for synthetic candidates only when a target name or
   path matches at least one meaningful hint.
4. Deduplicate again, then compute query matches for the full pool.

Hints never delete candidates from `--why` or JSON. They determine which
candidates enter the hinted finalist set and how those finalists rank.

### 11.8 Hinted ordering

Candidates with at least one accepted hint match sort by:

1. direct identity match present;
2. best identity quality descending;
3. identity points descending;
4. query coverage descending;
5. total query points descending;
6. scope points descending;
7. structural points descending;
8. the stable tie-breakers from §9.6.

This prevents a broad exact synonym such as `laravel` from tying every Laravel
action with a specific `participant-sync` identity match.

### 11.9 Automatic-execution gate for hints

Hints do not simply add points to structural score. They use a separate gate.

Let `M` be candidates with at least one accepted meaningful-term match.

```text
M is empty
  -> no hint match; never auto-run an unrelated unhinted default
  -> TTY: open unfiltered picker with a no-match explanation
  -> non-TTY: emit candidate data and exit 5

M is non-empty
  -> rank by §11.8
  -> form finalist set F from candidates that:
       (a) share the top candidate's highest match class,
       (b) are within IDENTITY_QUALITY_MARGIN of its best identity quality
           when that class is Identity, and
       (c) are within QUERY_POINTS_MARGIN of its total query points
```

The top candidate may auto-run only when all are true:

1. it is available;
2. it is not `Confirm`;
3. `F` contains exactly one candidate;
4. at least one meaningful hint matched;
5. either:
   - an identity matched at automatic quality (`>= 0.86` initially), or
   - the candidate is structurally `Automatic`, has an exact scope match, and
     is the clear structural winner within that scope;
6. if selection policy is `ExplicitHint`, the qualifying match is an identity,
   not merely program, framework, or description context.

Otherwise open the picker. `IDENTITY_QUALITY_MARGIN` and
`QUERY_POINTS_MARGIN` are named, corpus-tuned constants. Initial values of 40
quality-millis and 8 query points are reasonable starting hypotheses, not
compatibility guarantees.

This is intentionally different from “count every candidate having any strong
match.” Broad ecosystem hints create a scope; specific action/path hints decide
within that scope.

### 11.10 Explainability

Query evidence is displayed separately from structural evidence:

```text
Query match
  participant  -> ParticipantSyncTest.php  identity/exact       100
  laravel      -> artisan                  scope/synonym          60
  whatever     -> no match                                      0
  coverage: 2/3 meaningful terms

Structural evidence
  base suitability: Laravel test                                80
  artisan present                                                15
  same directory                                                 30
```

The execution preamble summarizes the decisive match:

```text
› php artisan test tests/Feature/ParticipantSyncTest.php
  matched: “participant” -> ParticipantSyncTest.php
```

### 11.11 Picker filter

When initial hints caused the picker to open, prefill its filter with those
hints. The picker uses the same normalization and matching implementation, not
a second fuzzy algorithm. Clearing the filter reveals the full candidate pool
already discovered for that invocation.

---

## 12. Resolution

### 12.1 Unhinted algorithm

```text
C = available candidates for requested intent after dedupe/scoring
A = candidates in C with SelectionPolicy::Automatic

if C is empty
    -> NoCandidates
if --pick
    -> picker with all candidates
if remembered choice is valid
    -> remembered candidate
if A is empty
    -> picker
if top(A).structural_points < AUTO_FLOOR
    -> picker with low-confidence explanation
if A has one candidate
    -> select it
if top(A) - second(A) > CLEAR_WINNER_MARGIN
    -> select top(A)
else
    -> picker
```

The number of low-tier candidates must not cause an otherwise unique canonical
candidate to become ambiguous, but those candidates remain visible in forced
picking and diagnostics.

### 12.2 Hinted algorithm

```text
expand candidate pool according to chaos
compute QueryMatch for every candidate
M = candidates with accepted meaningful matches

if --pick
    -> picker ordered by hinted rank
if remembered query-specific choice is valid
    -> remembered candidate
if M is empty
    -> picker/no-TTY ambiguity; never unhinted auto-fallback
rank M and form finalist set F
if hinted automatic gate passes
    -> top(M)
else
    -> picker
```

### 12.3 No candidates

Errors contain scan scope, truncation state, manifests, relevant diagnostics,
and actionable alternatives. Do not invent facts such as test counts.

```text
dev: nothing runnable found for Run in ./src

  package root:   ~/code/mylib
  workspace root: —
  scanned:        41 entries (complete)
  found:          Cargo.toml
  cargo:          crate `mylib` has no executable targets

  Try:
    dev test ./src
    dev build ./src
    dev run ./src --pick
```

### 12.4 Passthrough argument placement

Each candidate owns a function or declarative style that transforms opaque
user arguments into final argv. Representative styles:

```text
Append:       <base argv> <user argv>
DoubleDash:   <base argv> -- <user argv>
NpmRun:       npm run <script> -- <user argv>
Custom:       detector-owned pure argv transformation
```

Manager-specific behavior must be verified by integration tests. The user
writes only one delimiter separating `dev` hints from child arguments; any
additional delimiter required by the underlying tool is inserted by the
candidate.

---

## 13. Picker

### 13.1 Implementation

Use `ratatui` with `crossterm`. Use the shared hint matcher for interactive
filtering. A generic fuzzy-list widget is insufficient because evidence and
query coverage are first-class UI.

### 13.2 Layout

```text
┌─ dev run — 5 candidates in ~/code/myapp ───────────────────────────┐
│ query: participant laravel_                                        │
├────────────────────────────┬────────────────────────────────────────┤
│ ● Participant sync    196  │ php artisan queue:work participants    │
│   Laravel worker            │ cwd: ~/code/myapp                      │
│                             │                                        │
│   Participant test    184  │ Query match                            │
│   Laravel test              │ +100 participant -> action name       │
│                             │  +60 laravel -> detector synonym       │
│   Vite dev server     125  │ coverage 2/2                           │
│                             │                                        │
│   Compose up           80  │ Structural evidence                    │
│                             │  +80 declared Composer script          │
│ ⚠ seed.php             60  │  +30 same directory                    │
├────────────────────────────┴────────────────────────────────────────┤
│ ↵ run  ^R run+remember  ^D print  ^Y copy  tab details  esc cancel │
└─────────────────────────────────────────────────────────────────────┘
```

Scores are integer ranking points and MUST NOT be rendered as probabilities or
percentages.

### 13.3 Behavior

- Typing filters and re-ranks across the already discovered candidate pool.
- Initial CLI hints prefill the query.
- `Enter` runs once without remembering.
- `Ctrl-R` runs and remembers for the exact target/intent/query/chaos key.
- `Ctrl-D` prints the selected executable command and exits without running.
- `Ctrl-Y` copies a reproducible shell command when clipboard support is built.
- `Tab` toggles compact and full evidence.
- `Esc` or `Ctrl-C` restores the terminal and exits 130.
- Unavailable candidates are dimmed and selectable only to obtain their clean
  availability diagnostic; selection never attempts an obviously missing
  executable.

At narrow terminal widths, switch to a single-pane list and show details below
the selection. If terminal initialization fails, fall back to the non-TTY
candidate table rather than leaving the terminal altered.

### 13.4 Non-TTY behavior

When interaction is required and either stdin or stderr is not a TTY, do not
initialize the picker. Emit the ranked candidate table to stderr, or the JSON
resolution when requested, and exit 5.

`stdout` alone may be redirected while stderr and stdin remain interactive;
the picker may still run because child stdout belongs to the eventual command.

### 13.5 Exact command display

The UI has two different renderings:

- **Diagnostic argv display:** losslessly describes program and each argument,
  escaping control characters. It need not be copy-paste shell syntax.
- **Shell command rendering:** used by `--dry-run` and clipboard. It includes
  `cd`, environment changes, and shell-specific quoting.

Implement explicit POSIX-shell and PowerShell renderers. Do not claim one
quoted string is portable across shells. Non-UTF-8 Unix arguments that cannot
be represented faithfully as portable shell text must be diagnosed; execution
itself remains supported.

---

## 14. Remembered choices and cache

### 14.1 Semantics

Choices are remembered only after explicit `Ctrl-R` or an equivalent future
flag. Merely selecting with Enter does not change state. Therefore the product
promise is “the picker can be taught,” not “the picker always appears once.”

An unhinted default and a hinted target are independent. Remembering one test
file for query `participant sync` MUST NOT cause plain `dev test` to run only
that file.

### 14.2 Storage

Use `$XDG_STATE_HOME/dev/choices.json`, falling back to
`~/.local/state/dev/choices.json`. The format is versioned JSON, readable and
deletable by the user.

Writes require an exclusive cross-process lock around read-modify-write, then a
same-directory temporary file, flush, and atomic rename. Atomic rename alone
prevents corruption but does not prevent two writers from losing one another's
updates. If the lock cannot be acquired within a short bounded interval, skip
the cache write and continue execution with a warning.

### 14.3 Lookup key

The project shape is stored in the entry, not embedded in the map key. This is
necessary to locate and classify a stale entry.

```rust
pub struct CacheKey {
    pub physical_anchor: PathBuf,
    pub intent: Intent,
    pub query: QueryCacheKey,
    pub chaos: u8,
}

pub enum QueryCacheKey {
    Unhinted,
    Hinted(QueryHash),
}

pub struct CacheEntry {
    pub action_key: String,
    pub candidate_id: CandidateId,
    pub command_fingerprint: String,
    pub shape: ShapeSnapshot,
    pub cache_schema: u32,
    pub registry_fingerprint: String,
    pub matcher_schema: u32,
    pub chosen_at: SystemTime,
    pub last_used_at: SystemTime,
}
```

`QueryHash` is computed from deduplicated normalized hints sorted bytewise. Hint
order therefore does not create duplicate entries. `Hinted` remains distinct
from `Unhinted` even when all supplied words are filler; a filler-only query
must not reuse an unhinted automatic cache entry.

### 14.4 Fast shape snapshot

A cache hit should not require a full file walk. At selection time record:

- content digests for semantic manifests actually read;
- existence, size, and high-resolution mtime for lockfiles and root markers;
- metadata for the selected target and executable when project-local;
- metadata for watched parent directories whose entry changes could introduce
  or remove relevant manifests/configs;
- the sorted marker projection derived from the current detector registry;
- logical and physical roots;
- the registry fingerprint and matcher schema revision.

Validation stats watched paths, reads and hashes only the small semantic
manifest set, and compares directory metadata. Use a stable digest algorithm
such as BLAKE3 rather than a process-seeded standard hasher.

Directory mtimes and coarse filesystems are imperfect. `--no-cache`, `--pick`,
and `--forget` are explicit escape hatches. A benchmark should verify that the
snapshot is materially faster than scanning before preserving the 30 ms goal.

### 14.5 Stale entries

On a changed snapshot:

1. Run discovery again.
2. Look for the exact `candidate_id`.
3. If found, use the newly generated candidate and print a concise “project
   changed; remembered action still exists” note.
4. If only the semantic `action_key` exists but its command fingerprint changed,
   require the picker; do not silently accept new argv.
5. If the action disappeared, open the picker with the previous choice shown as
   unavailable context.

Never switch a remembered choice to a different action merely because it now
scores higher.

### 14.6 Maintenance

- `--forget` removes only the active lookup key.
- `dev cache list` shows target, intent, query, action, age, and stale status.
- `dev cache clear` clears all remembered choices after a confirmation on a
  TTY; `--yes` may be added for noninteractive use.
- Prune entries unused for 90 days on a successful write.
- Cap at 500 entries using `last_used_at` LRU order.

---

## 15. Execution

Execution is a separate boundary. Detectors construct argv; only the execution
module starts a process.

### 15.1 Unix

After terminal restoration and preamble output, prefer process replacement:

```rust
use std::os::unix::process::CommandExt;

let err = command.current_dir(cwd).envs(env).exec();
```

If `exec` returns, execution failed. Map `ENOENT`, `EACCES`, and other common
errors to concise diagnostics. Process replacement naturally preserves the
terminal, signals, and eventual exit status.

Do not add post-run reporting in v1. Any future feature requiring work after the
child exits must explicitly revisit the process model.

### 15.2 Windows

Windows has no `exec`. The child-process implementation MUST:

- start the child in a new process group;
- attach it to a Job Object configured with kill-on-close;
- forward console Ctrl-C and Ctrl-Break appropriately;
- inherit stdin/stdout/stderr and enable virtual terminal processing;
- return the child exit code;
- avoid killing intentionally detached processes only if a future candidate
  explicitly declares detachment semantics.

Windows behavior is release-blocking if the binary is advertised as supporting
Windows; it is not an acceptable permanent “manual verification” gap.

### 15.3 Child-process fallback

If Unix later needs a wrapper process:

- put the child in its own process group;
- forward `SIGINT`, `SIGTERM`, `SIGHUP`, and `SIGQUIT` to the group;
- wait after the first interrupt so the child can clean up;
- send `SIGKILL` after a second interrupt within three seconds;
- map signal termination to `128 + signal`;
- inherit file descriptors without buffering or transformation.

### 15.4 Program and PATH resolution

Resolve the candidate's program against its effective `PATH` without executing
it. Do not globally prepend `node_modules/.bin` or `vendor/bin`: doing so can
allow a project-local binary named `php`, `cargo`, or another runtime to shadow
the intended executable.

Use explicit ecosystem mechanisms instead:

- package scripts through the selected package manager;
- package-local binaries through a verified local-only manager exec;
- Composer tools through explicit `vendor/bin/<tool>` paths;
- directly anchored project executables through explicit paths.

Relative entries already present in the user's `PATH` follow platform process
semantics; `dev` does not rewrite them.

### 15.5 Execution preamble

Immediately before execution, print one concise line to stderr:

```text
› pnpm run dev  (vite, ~/code/myapp)
```

For hinted decisions, add the decisive match on a second dim line when useful.
Flush stderr before `exec`. `--quiet` suppresses the preamble. `NO_COLOR` or
`--color never` removes styling but does not suppress text.

### 15.6 Runtime environment

Inherit the caller's environment, then apply only detector-declared deltas.
`dev` MUST NOT load `.env` files itself. Native tools may do so as part of their
normal behavior.

### 15.7 Doctor probes

`dev doctor` derives its rows from the unique `ToolRegistration` values in the
detector registry. It MUST NOT maintain a second tool list. Rows have stable
`ToolId` ordering, and identical tool registrations shared by several detectors
collapse to one row.

There is no default version argument. Every tool explicitly registers one of:

- an audited command probe with exact argv and a tool-specific timeout;
- a bounded local-metadata reader;
- presence-only reporting with a reason that a safe version probe is not
  available.

Initial exceptions demonstrate why a generic `--version` fallback is
forbidden:

```text
go      -> go version
zig     -> zig version
just    -> just --version
flutter -> read SDK-local bin/cache/flutter.version.json
```

Most other tools may explicitly register `--version`, but that choice remains
visible in their own registration. Flutter's launcher is not a safe probe: on
a stale installation it may wait for a startup lock, update the embedded Dart
SDK, run `pub upgrade`, or rebuild the tool snapshot before printing anything.
The metadata reader resolves recognized SDK/FVM symlinks and reads bounded JSON.
If that layout is unavailable, doctor reports Flutter as present with an
unknown version; it does not execute the launcher as a fallback.

Command probes use null stdin and captured, bounded output. On timeout, terminate
the complete probe process scope where the host supports process groups or Job
Objects, then reap it. Render the timeout stored on that tool's probe rather
than a global constant. Doctor still must not install, update, source project
configuration, or access the network.

### 15.8 Exit codes

| Code | Meaning |
|---:|---|
| 0 | informational success or resolved dry run |
| 1 | internal error |
| 2 | usage error |
| 4 | no candidates |
| 5 | ambiguity or unmatched hints requiring interaction, but no TTY |
| 6 | selected candidate unavailable or unsupported on this host |
| 130 | picker cancelled |
| other | executed command's exit code |

On Unix process replacement means the child supplies its own exit status
directly.

---

## 16. Trust and safety boundary

Calling `dev run`, `dev build`, or `dev test` authorizes execution of the command
that resolution selects, including that command's documented runtime behavior.
It does not authorize discovery-time code execution or a separate install,
restore, update, or network step added by `dev`.

Requirements:

- Manifest parsing is data-only.
- Build-language files such as `Package.swift` and `build.zig` are never run to
  discover targets.
- Displayed commands are derived from the exact argv to be executed.
- No candidate is executed through a shell merely for quoting convenience.
- A cached candidate is bound to project identity and a validated shape.
- Symlinks are not followed during recursive discovery unless explicitly
  targeted.
- Missing and host-incompatible candidates do not auto-run.
- Fuzzy matching never lowers the automatic identity-quality threshold, even at
  the broadest chaos level.

The unavoidable race between final inspection and `exec` is treated like the
same race in a shell: the selected file may change. Recursion is rechecked at
the execution boundary.

---

## 17. State and configuration policy

V1 has no project-local `dev.toml`, executable hook, or task-definition format.
This preserves the product's zero-setup discovery constraint and avoids turning
`dev` into a second-rate Justfile.

The state file is not a task definition. It records explicit user choices and
may be deleted without affecting a project's behavior.

Do not phrase the absence of configuration as an eternal technical guarantee.
A future user-level alias layer, detector plugin API, or project override would
require a separate design proving that it does not silently execute repository
code or make zero-config projects second-class. No such mechanism exists in v1.

Runtime configuration is limited to CLI flags and standard environment inputs:
`PATH`, TTY state, `NO_COLOR`, and `XDG_STATE_HOME`.

---

## 18. Machine-readable interface

`--json` emits one versioned object and nothing else on stdout:

```json
{
  "schema_version": 1,
  "invocation": {
    "intent": "run",
    "target": "/home/h/code/app",
    "hints": ["wibblewbale"],
    "chaos": 1
  },
  "scan": {
    "package_root": "/home/h/code/app",
    "workspace_root": null,
    "scan_root": "/home/h/code/app",
    "structural_entries": 48,
    "target_entries": 12,
    "truncated": false
  },
  "resolution": {
    "status": "resolved",
    "selected_candidate_id": "...",
    "reason": "unique_identity_match"
  },
  "candidates": [],
  "diagnostics": []
}
```

Resolution status is one of:

```text
resolved
ambiguous
hint_no_match
no_candidates
remembered
```

Every candidate includes:

- ID, action key, detector, intent, origin, policy, and availability;
- structured program/argv values containing a human `display` string and, for
  non-UTF-8 Unix values, an optional base64 byte representation;
- working directory and environment deltas;
- structural and query ranks;
- structural/query evidence;
- lifecycle, label, and description.

JSON field removals or semantic changes require a schema-version increment.
New optional fields may be added compatibly.

---

## 19. Suggested crate layout

```text
dev/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── cli.rs
│   ├── intent.rs
│   ├── candidate.rs
│   ├── diagnostic.rs
│   ├── registry.rs
│   ├── doctor.rs
│   ├── scan/
│   │   ├── mod.rs
│   │   ├── roots.rs
│   │   ├── index.rs
│   │   ├── target_index.rs
│   │   └── manifest.rs
│   ├── detect/
│   │   ├── mod.rs
│   │   ├── builder.rs
│   │   ├── node.rs
│   │   ├── cargo.rs
│   │   ├── php.rs
│   │   ├── go.rs
│   │   ├── zig.rs
│   │   ├── swift.rs
│   │   ├── dart.rs
│   │   ├── python.rs
│   │   ├── just.rs
│   │   ├── make.rs
│   │   ├── docker.rs
│   │   └── shell.rs
│   ├── query/
│   │   ├── mod.rs
│   │   ├── normalize.rs
│   │   ├── matcher.rs
│   │   ├── rank.rs
│   │   └── synth.rs
│   ├── score.rs
│   ├── dedupe.rs
│   ├── resolve.rs
│   ├── cache/
│   │   ├── mod.rs
│   │   ├── key.rs
│   │   ├── shape.rs
│   │   └── lock.rs
│   ├── ui/
│   │   ├── picker.rs
│   │   ├── why.rs
│   │   ├── json.rs
│   │   ├── command_display.rs
│   │   └── error.rs
│   ├── exec/
│   │   ├── mod.rs
│   │   ├── unix.rs
│   │   └── windows.rs
│   └── path.rs
├── tests/
│   ├── fixtures/
│   ├── snapshots/
│   ├── cli_test.rs
│   ├── detect_test.rs
│   ├── query_test.rs
│   ├── cache_test.rs
│   └── exec_test.rs
└── benches/
    ├── scan.rs
    └── query.rs
```

### 19.1 Dependencies

Likely dependencies:

- `clap` with derive;
- `ignore`;
- `serde`, `serde_json`, `toml`, and YAML parsing;
- `smallvec`;
- `ratatui` and `crossterm`;
- `nucleo` and `strsim`;
- `blake3` for stable fingerprints;
- a small cross-platform advisory-lock crate;
- `directories`;
- `thiserror`, with `anyhow` optionally at the binary boundary;
- `arboard` only behind an optional clipboard feature.

Avoid `tokio`; no asynchronous runtime is needed. Avoid mandatory clipboard or
desktop dependencies in the minimal/static build. Pin dependency versions
compatible with the declared MSRV and test the MSRV in CI.

“Single binary” does not automatically mean statically linked on every target.
If static Linux artifacts are promised, build and test explicit musl targets.

---

## 20. Testing specification

### 20.1 Fixture corpus

Create at least 50 tiny, real-shaped repositories covering:

- npm, pnpm, Yarn, and Bun package scripts;
- Vite and Next with and without canonical scripts;
- named and unnamed workspace members;
- Cargo single bin, `default-run`, implicit `src/bin`, multi-bin, examples,
  library-only, and nested workspace globs;
- Laravel Composer `dev`, Artisan serve/test, plain Composer, and PHP files;
- Go root main, multi-file main, `cmd/`, module subtree, and `go.work`;
- Zig package and standalone file;
- Swift executable/library packages and bare Xcode projects;
- Flutter generated platforms and host incompatibility;
- standalone Python files with and without active virtual environment;
- Justfile case variants, private/default/aliased/dependency-only recipes,
  zero-minimum parameters, and bounded literal imports;
- Jake aliases, default/required-parameter handling, and namespaced imports;
- Taskfile spelling precedence, internal/required-var tasks, and includes;
- mise TOML task forms, aliases, hidden tasks, config layering, and includes;
- Sema package entrypoints, conventional `tests.sema`/`*.test.sema` suites,
  and standalone source files; tests execute as source files rather than a
  nonexistent `sema test` subcommand;
- Gradle Groovy/Kotlin markers, literal settings/task declarations, and cached
  versus uncached wrappers;
- Maven modules, packaging, known declared plugins, and cached versus uncached
  wrappers;
- `.sln`, `.slnx`, `.slnf`, and .NET project build/test/run selection;
- Make comments, wrapper targets, and syntax the scanner intentionally ignores;
- all four Compose filenames and Compose beside a native Vite app;
- shell shebang variants, missing executable bit, and extensionless executables;
- ambiguous Node + PHP + shell directory;
- empty directory and malformed manifests;
- symlinked target and non-UTF-8 path/argv fixtures on Unix.

### 20.2 Structural golden tests

Snapshot for each fixture and intent:

- roots and scan scope;
- candidates in stable order;
- base, evidence, proximity, and total points;
- selection policy and availability;
- deduplication result;
- diagnostics;
- final resolution reason.

Run detection repeatedly and assert byte-identical JSON. Score changes must show
their ranking consequences in reviewable snapshot diffs.

### 20.3 Query corpus

Table-driven positive cases must cover:

- separator drift: `wibble_wabble`, `wibble.wabble`;
- camel-case drift: `WibbleWabble`;
- substitutions and transpositions: `wibblewbale`;
- truncation and prefixes: `wibble`;
- valid acronyms: `ww`, `ps` for participant-sync;
- ecosystem scopes: `laravel`, `rust`, `vite`;
- combined scope and identity: `laravel participant`;
- filler: `whatever`, `thing`, `in`;
- target paths nested deeper than structural depth;
- two identically named actions in different workspace members.

Negative cases matter more than easy positives:

- unrelated same-length tokens;
- two-character substring explosions (`go`, `js`, `rs`);
- transposition matcher false positives;
- one broad framework hint matching several actions;
- all meaningful hints unmatched;
- one exact scope match but no identity match;
- chaos 2 weaker matches never bypassing the auto gate.

Assertions must cover query evidence, coverage, finalist set, and whether the
outcome auto-runs, picks, or returns hint-no-match.

### 20.4 Property and invariant tests

- Candidate order is independent of detector registration order.
- Every detector/source/tool ID is registered exactly once or shared with an
  identical declaration.
- Derived root markers, cache marker projections, doctor rows, target hooks,
  and error marker names contain every applicable registration without manual
  side tables.
- Candidate builders reject an unregistered source/tool and incomplete
  evidence/search metadata.
- Deduplicating twice is idempotent.
- Adding unrelated low-tier candidates cannot change an unhinted clear winner.
- Adding an unmatched hint cannot silently select an unrelated candidate.
- Increasing chaos may add candidates but cannot relax the automatic identity
  threshold.
- `ExplicitHint` never auto-runs from only a scope/context match.
- `Confirm` never auto-runs.
- Normalization is stable and duplicate hints do not change rank.
- `dev run test` parses identically whether `./test` exists or not.
- No detector has access to the execution interface.

### 20.5 Cache tests

- Unhinted and hinted choices are independent.
- Query order normalizes to one key.
- Chaos levels do not accidentally share entries.
- Manifest content changes invalidate a snapshot.
- Adding a root config invalidates through watched-directory metadata.
- Adding, removing, or case-renaming any registered exact, case-insensitive, or
  extension marker invalidates the relevant snapshot.
- Changing a detector's candidate schema changes the registry fingerprint.
- Stale exact candidate persists with a note.
- Same action key with changed argv requires confirmation.
- Concurrent writers preserve both entries.
- Corrupt state is quarantined or ignored with a warning, never fatal to run.

### 20.6 Execution tests

Unix integration tests must verify:

- stdin/stdout/stderr inheritance;
- colors and TTY detection;
- exact argv including spaces, quotes, empty arguments, and non-UTF-8 values;
- working directory and environment deltas;
- signal delivery and exit status;
- recursion protection;
- no project-local binary shadows `php`, `cargo`, or another runtime through
  implicit PATH rewriting.

Equivalent Windows tests cover Job Objects, Ctrl-C, exit codes, and quoting.

### 20.7 Doctor tests

- Assert exact argv for every command probe; in particular Go and Zig use the
  `version` subcommand while Just uses `--version`.
- Assert tool-specific timeout rendering and complete process-scope cleanup.
- Assert Flutter reads bounded local metadata without launching Flutter.
- Assert a missing or unrecognized metadata layout reports presence without an
  unsafe command fallback.
- Assert doctor rows are the stable deduplicated projection of registered tools.

### 20.8 Performance tests

Benchmark release builds against committed generated corpora:

- exact remembered hit;
- cold structural scan;
- chaos-1 conventional target scan;
- chaos-2 capped broad scan;
- query matching across 10, 100, 1,000, and 10,000 candidates;
- startup with and without optional clipboard support.

Record p50 and p95. Performance regression thresholds should be generous enough
to avoid flaky CI but strict enough to catch accidental subprocesses or full
repository walks on the structural path.

---

## 21. Milestones

### M1 — semantic spine

- CLI parsing and stable positional disambiguation.
- Logical/physical path handling and package/workspace root resolution.
- Structural index, diagnostics, candidate model, integer scoring, policy,
  dedupe, and deterministic resolution.
- Query normalization, exact matching, fuzzy adapter boundary, and hinted
  automatic gate.
- Node and Cargo detectors only.
- `--why`, `--list`, and versioned `--json`.
- Full fixture/snapshot harness; no execution.

Hints belong in M1 because they alter the candidate model and resolver rather
than decorating the picker.

### M2 — execution correctness

- Unix process replacement.
- Passthrough transformations and command rendering.
- Availability, recursion, preamble, and exit behavior.
- Integration tests for npm/pnpm/Yarn/Bun and Cargo passthrough.

At M2, `dev` is useful for Node and Rust without a TUI or cache.

### M3 — picker and remembered choices

- Ratatui picker using the shared matcher.
- Cache key, fast shape snapshot, lock, atomic write, and stale behavior.
- Non-TTY and dry-run resolution paths.

### M4 — common ecosystems

- Composer, Artisan, PHP, Go, Make, shell, Docker/Compose.
- Conventional target index for chaos 1.
- Synthetic candidate runner registry for PHP, JS, Python, and shell.

### M5 — broader matching and long tail

- Chaos 2 broad scan.
- Zig, Swift, Flutter/Dart, and additional target runners.
- `dev doctor`, cache inspection, optional clipboard.

### M6 — platform completion

- Windows Job Object/process behavior and integration tests.
- Cross-platform path, quoting, cache-lock, and host-availability behavior.
- Static artifact verification for platforms where static builds are promised.

### M7 — ecosystem expansion

- Static capability registry, registry-bound candidate builder, and removal of
  detector/tool/marker side tables.
- Justfile, Jake, Taskfile, and mise detectors and declared project-task
  dominance.
- Sema package and source-file support without doctest inference.
- Full Python project design.
- Gradle, Maven, and .NET detectors under the no-subprocess discovery contract.
- CMake, Bazel, Nix, and plugin feasibility.

---

## 22. Open decisions

1. **Required intent.** V1 requires Run/Build/Test. After usage data, decide
   whether bare `dev` means Run and whether `dev <word soup>` searches all
   intents.
2. **Query thresholds.** Tune identity auto quality and finalist margin against
   negative corpus cases, not demos that are easy to make pass.
3. **Chaos branding.** `--chaos` accurately conveys breadth but may be paired
   with a sober alias such as `--search-depth` for documentation and scripts.
4. **Python priority.** Decide whether detector order targets the maintainer's
   daily stack or general-market coverage.
5. **Cache fast path.** Validate directory metadata behavior on macOS, Linux,
   Windows, and common network filesystems before treating it as authoritative.
6. **Config policy.** V1 has no project config. Revisit only with an explicit
   product decision, not piecemeal detector escape hatches.
7. **Release builds.** `dev build` invokes ecosystem defaults. If users expect
   deployable/release artifacts, that should become a separate intent rather
   than ecosystem-specific hidden flags.

---

## Appendix A — Material differences from revision 3

Revision 4 makes detector registration the expansion boundary:

| Revision 3 | Revision 4 |
|---|---|
| Separate detector, root-marker, cache-marker, doctor-tool, target-hook, and cached-name lists | One static capability registry projected into each subsystem |
| Candidate construction is public and manually repeats detector defaults | Registry-bound builder validates sources, tools, evidence, availability, and search metadata |
| Detector strings double as implementation identity, display source, cache allowlist, and dedupe priority | Stable detector, candidate-source, and tool IDs have distinct roles |
| One `--version` command and timeout is applied to every doctor tool | Every tool declares an exact audited command, local metadata reader, or presence-only probe |
| Workspace expansion for Cargo, Node, and Go lives in the generic scanner | Optional pure workspace contributions live with their ecosystem registrations |
| Declared wrappers compete with lower-level defaults only through scores | Canonical project-task facades demote same-scope tool defaults without hiding them |
| Justfiles are indexed incidentally but have no consumer | Static Justfile parsing emits public zero-arity recipes and registers Just markers/tooling |
| Other declared task systems require ad hoc support | Jake, Taskfile, and mise use the same bounded facade parser contract and registered doctor probes |
| Sema source and package metadata are unknown | Static Sema candidates use declared entrypoints and ordinary `sema test` only |

The registry remains compile-time and data-only. Revision 4 does not introduce
runtime plugins or grant detectors permission to execute native task-listing
commands.

Migrate without a flag day:

1. Add registry IDs/descriptors and mirror current behavior while existing side
   tables still assert equal projections.
2. Replace cached-name restoration, dedupe specificity, root markers, cache
   markers, error markers, target hooks, and conventional roots with registry
   projections; remove each old table in the same commit as its replacement.
3. Introduce `CommandLayer` and the registry-bound candidate builder, then move
   existing detectors in small ecosystem groups.
4. Move doctor to registered probes and land the Go, Zig, Flutter, and Just
   probe corrections together.
5. Add the static Justfile, Jake, Taskfile, mise, and Sema parsers plus
   facade-dominance fixtures.
6. Add Gradle, Maven, and .NET only after registry extension tests prove no
   scanner, cache, UI, or doctor side table must change.

---

## Appendix B — Material differences from revision 2

Revision 2 correctly moved hints into M1 and added normalization, typo matching,
synonyms, discovery-tier participation, synthetic files, picker prefill, and
query-specific caching. Those ideas are preserved, with these changes:

| Revision 2 | Revision 3 |
|---|---|
| First positional becomes a path when a same-named entry exists | Only lexically path-like tokens or `--at` are paths, so ambient files cannot change parsing |
| Structural score is a clamped float assembled inconsistently from base values and evidence | Unbounded integer structural points with an explicit base and recomputation after dedupe |
| Low score doubles as “picker only” | Selection policy is explicit: Automatic, ExplicitHint, or Confirm |
| Any candidate with any strong hint enters set `S`; two members always force a picker | Identity, scope, and context are ranked separately; a specific identity can win inside a broad framework scope |
| A unique strong match runs regardless of structural tier | ExplicitHint actions require a direct identity match; Confirm actions never auto-run |
| Completely unmatched hints fall through and may auto-run an unrelated default | Unmatched meaningful hints never auto-run; they open the picker or return exit 5 |
| All hints are equally meaningful | Filler terms have zero coverage weight; short ambiguous terms have strict boundary rules |
| Fixed `0.60` quality threshold for all lengths and strategies | Length-sensitive calibrated thresholds; chaos can widen display but not automatic execution |
| Synthetic extension table can choose `npx tsx`, potentially installing from the network | Typed runner registry; TypeScript requires an already available local runner |
| Depth-3 index is also expected to find arbitrary hinted files | Separate shallow structural and broader target indexes with explicit chaos levels |
| Nearest root marker is the scan root | Separate package and workspace roots, fixing monorepo member discovery |
| Shape is embedded in the cache key | Cache key finds the prior entry; shape is stored and validated inside it |
| Shape validation still requires the scan that the cache is meant to avoid | Watched manifest/directory snapshot provides a real fast path |
| Atomic rename is the whole concurrency strategy | Cross-process read-modify-write lock plus atomic replacement |
| Project local bin directories are prepended globally to PATH | Explicit package-manager/local-tool invocation prevents runtime shadowing |
| Candidate command identity omits env and passthrough behavior | Candidate ID includes environment and passthrough semantics |
| `String` argv | `OsString` execution model preserves non-UTF-8 Unix arguments |
| Detector parse failures disappear into empty candidate sets | Detectors return candidates plus structured diagnostics |
| “No config ever” is an eternal constraint | No task-definition config in v1; future changes require an explicit product design |

The central conceptual change is that revision 3 keeps three questions separate:

1. **Structural suitability:** is this normally the right action here?
2. **Query relevance:** how closely did the user's hints identify it?
3. **Execution eligibility:** is this class of candidate allowed to auto-run?

Conflating those questions is what caused most of revision 2's threshold and
safety contradictions.

---

## Appendix C — Worked resolutions

### C.1 Broad scope plus specific identity

```console
dev test laravel participant sync
```

Candidates include Laravel's whole test suite and several target test files.
`laravel` matches their common scope. `participant` and `sync` match one target
identity. The target-specific candidate wins without every Laravel candidate
entering an undifferentiated strong-match set.

### C.2 Unmatched words

```console
dev run purple monkey lasagna
```

No meaningful hint matches. Even if Vite is the unhinted structural winner,
`dev` does not run it. On a TTY it opens the unfiltered picker with a no-match
header. In CI it exits 5 with candidates in JSON/stderr.

### C.3 Explicit obscure script

```console
dev run refresh-search-indxe
```

An npm script `refresh-search-index` is discovery-tier and normally invisible
to automatic resolution. The typo-tolerant identity match is unique and above
the automatic quality threshold, so its `ExplicitHint` policy permits it to
run. The exact package-manager command appears in the preamble.

### C.4 Same action in two packages

```console
dev run worker
```

Both `apps/api` and `apps/billing` expose `worker`. Their identity ranks tie, so
the picker opens. Adding `billing` makes that package scope the unique finalist:

```console
dev run billing worker
```

### C.5 Broad search discovers a loose target

```console
dev run --chaos 2 legacy importer
```

The broad target index finds `tools/legacy_importer.php`. The PHP runner creates
a synthetic `ExplicitHint` candidate. It may auto-run only if the file identity
match clears the same automatic quality threshold; a match found only in its
description or parent ecosystem opens the picker.
