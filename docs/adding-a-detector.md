# Adding a detector

This guide describes the complete path for adding an ecosystem to `dev`. It is
written so a coding agent can follow it in order, while keeping the reasoning
visible to a human reviewer.

Read [`AGENTS.md`](../AGENTS.md) and the detector, pipeline, registry, and test
sections of [`spec.md`](spec.md) before editing code. The specification is
normative when this guide is incomplete.

## The detector contract

A detector turns bounded, local project evidence into one or more fully known
commands. It does not execute the project's configuration to ask what commands
exist.

During discovery, a detector must not:

- spawn subprocesses, including native task-listing or help commands;
- access the network or construct a fallback runner invocation that may
  download missing software;
- write to the project;
- source shell code or evaluate executable build configuration;
- recursively walk the filesystem outside the shared indexes; or
- generate a shell command string.

Candidates contain a program and an argument vector (`argv`). Missing tools
remain visible as unavailable candidates; they are not installed or silently
removed.

The capability registry in [`src/registry.rs`](../src/registry.rs) is the
single source of truth. Registering a detector there automatically feeds:

- detector dispatch;
- package and workspace root resolution;
- structural and chaos-level scan expansion;
- candidate source metadata and availability;
- target binders and standalone target runners;
- `dev doctor` tool probes;
- cache shape, ambient environment, and registry fingerprinting; and
- no-candidate diagnostics.

Do not add a second detector-name, marker, tool, or doctor-probe table elsewhere.

## 1. Define the supported behavior first

Before writing code, answer these questions in the issue or pull-request
description:

1. Which local files prove this is the ecosystem?
2. Which `run`, `build`, and `test` commands can be constructed without
   executing project code?
3. Which actions are canonical enough for automatic selection, and which need
   an explicit hint or confirmation?
4. What are the exact argv, working directory, passthrough, environment, and
   lifecycle semantics?
5. Does the ecosystem have workspaces, standalone runnable files, or commands
   that bind to a selected test target?
6. Can a project wrapper download software? If so, what local evidence proves
   its declared distribution is already installed?
7. Which environment variables can change detection or wrapper selection?
8. What exact, bounded version probe belongs in `dev doctor`?

Prefer extending an existing detector when the new behavior belongs to the
same ecosystem. One implementation may expose several candidate sources—for
example, the Node detector emits `node`, `vite`, and `next` candidates.

Useful implementations to study:

| Need | Example |
|---|---|
| Manifest-backed defaults and a standalone target | [`zig.rs`](../src/detect/zig.rs) |
| Declared package scripts | [`composer.rs`](../src/detect/composer.rs) |
| Project-facade task parsing | [`just.rs`](../src/detect/just.rs) and [`task_facade.rs`](../src/detect/task_facade.rs) |
| Workspace classification and expansion | [`go.rs`](../src/detect/go.rs), [`gradle.rs`](../src/detect/gradle.rs), or [`maven.rs`](../src/detect/maven.rs) |
| Multiple sources and target binding | [`node.rs`](../src/detect/node.rs) |

## 2. Add the detector module

Create `src/detect/<ecosystem>.rs`, implement `Detector`, then declare and
re-export it from [`src/detect/mod.rs`](../src/detect/mod.rs):

```rust
mod example;

pub use example::ExampleDetector;
```

The detector receives a read-only `ScanCtx`:

- `context.invocation` — intent, target, hints, passthrough, and chaos level;
- `context.roots` — logical anchor, package root, workspace root, and scan root;
- `context.index` — bounded structural and optional target entries; and
- `context.index.manifests` — bounded, memoized text reads.

Use `context.index.all_entries()` instead of starting another filesystem walk.
Read semantic files through `context.index.manifests.read(path)` instead of
`std::fs::read_to_string`. Turn malformed or unsupported input into a
detector-owned `Diagnostic`, continue with independent files, and keep output
deterministic with sorted paths or `BTreeMap`/`BTreeSet` where ordering can
escape the detector.

A minimal manifest-backed shape looks like this. It is an outline, not a
substitute for verified ecosystem semantics:

```rust
use std::path::Path;

use crate::candidate::{Evidence, EvidenceKind, SearchDocument, SelectionPolicy};
use crate::intent::Intent;
use crate::registry::{EXAMPLE_SOURCE, EXAMPLE_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

pub struct ExampleDetector;

impl Detector for ExampleDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        if context.invocation.intent != Intent::Build {
            return Detection::default();
        }

        let mut output = Detection::default();
        for entry in context.index.all_entries().filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "example.toml")
        }) {
            let manifest = context.roots.scan_root.join(&entry.relative_path);
            let directory = manifest.parent().unwrap_or(Path::new(".")).to_path_buf();
            let scope = entry
                .relative_path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .map_or_else(
                    || ".".to_owned(),
                    |path| path.to_string_lossy().replace(['/', '\\'], ":"),
                );
            let description = "Builds the declared Example project";

            CandidateBuilder::tool_default(
                EXAMPLE_SOURCE,
                Intent::Build,
                directory.clone(),
                "build",
            )
            .action_key(format!("example:{scope}:build"))
            .tool(EXAMPLE_TOOL)
            .args(["build"])
            .cwd(directory)
            .selection(SelectionPolicy::Automatic)
            .base_points(95)
            .evidence(Evidence {
                kind: EvidenceKind::Manifest,
                reason: "example.toml declares an Example project".to_owned(),
                points: 0,
                source: Some(entry.relative_path.clone()),
            })
            .search(SearchDocument {
                identities: vec!["build".to_owned()],
                target_paths: vec![entry.relative_path.clone()],
                scopes: vec![scope],
                text: vec![description.to_owned()],
                ..SearchDocument::default()
            })
            .label("Example build")
            .description(description)
            .emit(&mut output);
        }
        output
    }
}
```

The real implementation should parse any manifest fields that affect command
construction through `DiscoveryFiles`, validate relative paths before joining
them to a project root, and report ignored unsafe values.

## 3. Register every capability once

Add detector, candidate-source, and tool IDs near the corresponding constants
in [`src/registry.rs`](../src/registry.rs). Add an exact doctor probe, then one
`DetectorRegistration` to `REGISTRATIONS`.

```rust
pub const EXAMPLE: DetectorId = DetectorId::new("example");
pub const EXAMPLE_SOURCE: CandidateSourceId = CandidateSourceId::new("example");
pub const EXAMPLE_TOOL: ToolId = ToolId::new("example");

const EXAMPLE_PROGRAM: ToolRegistration =
    command_tool(EXAMPLE_TOOL, "example", &["--version"]);
```

```rust
DetectorRegistration {
    id: EXAMPLE,
    candidate_sources: &[CandidateSourceRegistration {
        id: EXAMPLE_SOURCE,
        metadata_priority: 2,
        default_tags: &["example"],
    }],
    synonyms: &["example", "ex"],
    markers: &[ProjectMarker {
        pattern: MarkerPattern::Exact("example.toml"),
        root_role: RootRole::Package,
    }],
    tools: &[EXAMPLE_PROGRAM],
    conventional_roots: ROOTS,
    cache_environment: &[],
    candidate_schema: 1,
    detector: &ExampleDetector,
    workspace: None,
    target_binders: &[],
    target_runners: &[],
},
```

Registration order is not precedence and must not change results.

### Registration fields

| Field | Meaning |
|---|---|
| `id` | Stable implementation identity. Do not reuse a display label. |
| `candidate_sources` | User-visible source identities, metadata priority, and default search tags. Specialized sources may share one detector. |
| `synonyms` | Ecosystem terms used by hinted matching. Keep them specific and evidence-based. |
| `markers` | Files that participate in root resolution, diagnostics, and cache invalidation. |
| `tools` | Programs the detector may place in candidates, with their exact doctor probes. |
| `conventional_roots` | Ecosystem-specific directories eligible for chaos-1 target discovery. Reuse `ROOTS` unless the ecosystem proves additional names. |
| `cache_environment` | Ambient variables whose values can change emitted candidates or wrapper choice. |
| `candidate_schema` | Per-detector semantic version used by the registry fingerprint. Start at `1`; bump it when hook behavior changes in a way static metadata does not describe. |
| `workspace` | Optional pure-data package/workspace classification and member-scan contribution. |
| `target_binders` | Optional hooks that specialize an existing candidate for a selected file. |
| `target_runners` | Optional hooks that create a command for a standalone selected file. |

Candidate-source metadata priority follows the existing convention: generic
file targets are normally `1`, ordinary ecosystems `2`, and specialized
project facades or framework sources `3`. Match a neighboring detector; do not
raise priority to force a desired winner.

### Marker roles

| Role | Use it when |
|---|---|
| `Package` | The marker unconditionally declares a package root. |
| `Workspace` | The marker unconditionally declares a workspace root. |
| `Classified` | The same marker may be a package, workspace, both, or neither after bounded parsing. This requires a `WorkspaceContributor`. |
| `Auxiliary` | The file affects project shape or tool choice but does not establish a root by itself. |

Use `Exact` for a fixed basename, `AsciiCaseInsensitiveBasename` only when the
ecosystem specifies case-insensitive names, `BasenamePrefixSuffix` for a
bounded family such as mise environment files, and `Extension` for true
project markers such as Sema source files. A standalone `TargetRunner` does not
need an extension marker; Python uses this path to avoid treating every source
file as a package root. Do not encode directory separators inside an exact
marker.

### Doctor probes

The doctor row is derived from the tool registration. Do not add a second tool
list to `doctor.rs`; ordinary command probes require no detector-specific
doctor logic.

- Use the tool's documented version command, not a universal `--version` guess.
- Select a meaningful line with `CommandOutput::LinePrefix` when the first line
  is a banner or separator.
- Keep probes bounded and side-effect free.
- Use local metadata or presence-only probes only when invoking the tool is
  demonstrably unsafe or too slow. A new local-metadata probe requires a small,
  audited implementation in `doctor.rs` and focused tests.
- Shared tools are allowed only when their registrations are identical.

## 4. Construct candidates through `CandidateBuilder`

Choose the builder constructor that describes the command's relationship to
the project:

| Constructor | Layer | Typical use |
|---|---|---|
| `project_facade` | `ProjectFacade` | Just, Jake, Taskfile, mise, or a literal Make target that wraps project workflow. |
| `ecosystem_task` | `EcosystemTask` | A declared npm or Composer script. |
| `tool_default` | `ToolDefault` | A native ecosystem convention such as `cargo test` or `dotnet build`. |
| `direct_target` | `DirectTarget` | An explicit or hinted runnable file. |

Every candidate needs:

- a stable semantic `action_key` independent of labels, scores, and filesystem
  iteration order;
- a registered tool or an explicitly justified fixed program path;
- exact argv as `OsString` values, never a shell string;
- logical `cwd` and, when different, the project/member `scope_root` used for
  proximity;
- the correct passthrough style and lifecycle;
- `Declared`, `Conventional`, or `Synthetic` origin;
- an evidence-backed selection policy and base score;
- at least one evidence item with a useful reason and source; and
- searchable identities, target paths, scopes, tags/text, label, and
  description.

Use selection policies conservatively:

| Policy | Meaning |
|---|---|
| `Automatic` | Canonical, safe default that may run without a hint when it is the clear winner. |
| `ExplicitHint` | Valid alternative that requires matching user intent or picker selection. |
| `Confirm` | Action whose side effects or uncertainty require explicit confirmation. |

Do not suppress a candidate merely because its tool is absent. The builder
resolves availability and retains the missing program in `--why` and JSON. Do
not manually add proximity or availability evidence; shared scoring does that.

When changing an existing action's argv, cwd, environment, or passthrough
semantics, consider remembered choices: the command-derived candidate ID will
change. Keep `action_key` stable only when it remains the same conceptual
action, and bump `candidate_schema` when the registry metadata alone does not
capture the semantic change.

## 5. Add advanced hooks only when needed

### Workspaces

Implement `WorkspaceContributor` beside the detector when a manifest declares
members or needs package/workspace classification:

- `classify_root(marker, files)` parses only the marker and returns
  `RootClassification`.
- `scan_contribution(root, files)` returns deterministic include/exclude globs
  for declared members.
- All reads go through `DiscoveryFiles`.
- The shared scanner applies ignore rules, depth, and hard caps; the hook does
  not walk directories.

Register the hook in `workspace`. A `Classified` marker without a contributor
is rejected by registry validation.

### Target binding and standalone targets

Use a `TargetBinder` when a file specializes an existing project command, such
as binding a package test command to one test file. Use a `TargetRunner` when a
file is independently runnable, such as a standalone script.

Implement the hook beside the owning detector and register it in
`target_binders` or `target_runners`. Do not add another central extension or
filename table. Hooks receive indexed entries and must remain deterministic and
data-only.

### Project wrappers

Wrappers such as Gradle and Maven may download distributions. A new wrapper
integration must prove the declared distribution is already extracted in a
documented local cache layout before selecting the wrapper. Custom cache paths,
download-forcing flags, or unresolved environment/JVM overrides must make the
detector fall back to an already available global tool or retain an unavailable
candidate. Never run the wrapper during detection to find out.

## 6. Test the vertical slice

Detector support is not complete when the module compiles. Cover the layers the
feature changes.

### Detector unit tests

Put `#[cfg(test)] mod tests` at the bottom of the detector module. Use fresh
temporary directories and cover:

- each supported positive intent and exact argv/cwd/passthrough semantics;
- malformed and oversized manifests with useful diagnostics;
- ignored, private, parameterized, unsafe, or unsupported declarations;
- duplicate and precedence behavior;
- workspace includes/excludes and root classification, if applicable;
- missing tools and unsafe/uncached wrappers; and
- deterministic results independent of input order.

This project does not use doctests. Behavioral examples belong in unit or
integration tests.

Run `just detector-check` while editing detector code. This named CI gate
checks that every `Detector`, `WorkspaceContributor`, `TargetBinder`, and
`TargetRunner` implementation is registered exactly once. It also rejects
subprocess, network, write, unbounded-walk, and unaudited direct-read APIs in
production detector code; checks ambient environment reads against cache
metadata; and validates required registration metadata. The gate enforces
architectural boundaries, not detector semantics, so the behavioral tests
below remain required.

### Structural corpus

Add a small, numbered repository under
[`tests/fixtures/corpus/`](../tests/fixtures/corpus). Include only files that
drive discovery. The corpus exercises all three intents and validates complete
candidate metadata.

Regenerate the golden only after the implementation is correct:

```console
DEV_UPDATE_GOLDENS=1 cargo nextest run --locked structural_corpus_matches_golden_and_is_repeatable
git diff -- tests/snapshots/corpus-structural.snap
cargo nextest run --locked structural_corpus_matches_golden_and_is_repeatable
```

Read the diff candidate by candidate. A surprising score, root, policy, argv,
or resolution is a bug to investigate, not a snapshot to accept.

### CLI and query integration

Add built-binary coverage in [`tests/cli_test.rs`](../tests/cli_test.rs) for
user-visible selection, ambiguity, diagnostics, JSON, passthrough, or doctor
behavior. Use temporary projects and controlled `PATH` stubs. Add cases to
[`tests/query_corpus_test.rs`](../tests/query_corpus_test.rs) when introducing
new identities, synonyms, or typo-sensitive behavior.

For a local end-to-end inspection, use a real fixture with a non-executing
mode:

```console
cargo run -- run --why --no-cache --at /path/to/fixture hint
cargo run -- test --json --no-cache --at /path/to/fixture
```

Do not install the fixture's dependencies merely to make discovery pass, and
do not claim execution coverage from `--why` or JSON output.

### Registry, doctor, cache, and performance

- Registry validation runs in the normal suite. Update explicit tool-count or
  cache-environment expectations when the new registration intentionally
  changes them.
- Add doctor output-selection tests for unusual version banners or local
  metadata probes.
- Test cache invalidation when manifest content or ambient environment affects
  emitted commands.
- Run the release benchmark when the detector changes scan breadth, manifest
  reading, registry projections, query volume, or startup work.

## 7. Verify before handing off

```console
just qa
just bench   # required for measured paths
git diff --check
```

Then inspect the full diff and report verification by layer: unit, fixture,
CLI, platform, real-project `--why`, execution, or benchmark. Do not describe a
weaker layer as stronger evidence.

Before considering the detector complete, confirm:

- registration is the only new source of detector/tool/marker metadata;
- discovery performs no subprocess, network, install, write, or unbounded walk;
- every command is derived from local evidence and uses exact argv;
- automatic candidates are genuinely canonical and safe;
- malformed input degrades to diagnostics without blocking other detectors;
- cache inputs cover every manifest and environment value that changes output;
- tests include a plausible wrong implementation they would catch; and
- README/spec support tables are updated when the ecosystem becomes user
  visible.
