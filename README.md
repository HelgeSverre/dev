<p align="center">
  <img src="website/logo.svg" width="96" height="96" alt="dev Command Aperture logo">
</p>

<h1 align="center">dev</h1>

<p align="center">
  <strong>Run the right command.</strong><br>
  Zero-setup command discovery and launching for software projects.
</p>

<p align="center">
  <a href="https://github.com/HelgeSverre/dev/blob/main/docs/spec.md">Specification</a>
  ·
  <a href="website/index.html">Website</a>
</p>

`dev` finds commands a project already knows how to run, ranks the candidates
deterministically, and executes the best match with the original process
semantics intact. It is designed to provide IDE-style run configurations from
one fast command-line tool without introducing another task-definition format.

```console
dev run
dev build ./apps/web
dev test ./tests/Feature entitlement participant
dev run laravel queue
```

## Install

`dev` requires Rust 1.85 or newer. It is not yet published to a registry;
install from the repository:

```console
cargo install --git https://github.com/HelgeSverre/dev.git
```

For development or contributing, clone the checkout:

```console
git clone https://github.com/HelgeSverre/dev.git
cd dev
just install
dev completions --install
dev doctor
```

`just install` runs `cargo install --path .`. Remove either installation with:

```console
cargo uninstall dev-launcher
```

## How it works

- Discover commands from manifests, conventional files, and runnable targets.
- Match incomplete or misspelled hints without inventing shell commands.
- Select a clear result automatically and open a picker when the choice is
  ambiguous.
- Explain discovery, scoring, and resolution decisions through human-readable
  and JSON output.
- Remember explicit choices per project shape while keeping results
  deterministic.
- Preserve argv, environment, working directory, terminal I/O, signals, and
  exit status when executing a command.

Discovery is local, bounded, and read-only. It does not run project code,
access the network, install missing tools, or generate arbitrary shell text.
The selected command can still require dependencies that are not installed;
use the project's normal setup command when that happens. `dev` deliberately
does not insert `npm install`, `composer install`, `dotnet restore`, or similar
steps.

## Architecture

The capability registry is the single source of truth for discovery behavior,
project markers, tool availability, doctor probes, and cache identity.

```text
Capability registry
├── markers + workspace hooks ──► root resolution and bounded indexes
├── detectors + sources + tools ─► static candidate discovery
├── schema + environment keys ───► cache identity
└── tool probes ─────────────────► dev doctor

run / build / test
        │
        ▼
  root resolution
        │
        ▼
remembered-choice lookup
   ├── valid fast hit ──► process executor ──► child exit status
   │
   └── miss or inspection mode
                    │
                    ▼
         bounded file indexes
                    │
                    ▼
            static detectors
                    │
                    ▼
         candidates + diagnostics
                    │
                    ▼
    deduplicate + availability + scoring
                    │
                    ▼
         deterministic resolution
             │       │       │
             │       │       └── clear winner ──┐
             │       └── picker ── remember? ───┤
             └── why / list / JSON ──► explain  │
                                                 ▼
                                         selected command
                                           │           │
                                     dry-run           run
                                           │           │
                                           ▼           ▼
                                      print only   process executor
                                                       │
                                                       ▼
                                                child exit status
```

## Usage

```text
dev <run|build|test> [options] [target] [hint ...] [-- passthrough ...]
```

Targets must be path-like, such as `./apps/web` or `../service`. Other words
are retrieval hints. This keeps parsing stable regardless of which files happen
to exist in the current directory. Use `-C` or `--at` when an explicit path is
clearer:

```console
dev test --at ./apps/api participant
```

Inspect resolution without executing anything:

```console
dev run --list
dev run vite frontend --why
dev test --json
dev build --dry-run -- --release
```

Arguments after `--` are forwarded to the selected command without shell
re-parsing. `--why`, `--list`, `--json`, and `--dry-run` never execute the
candidate.

When hints are present, `dev` uses typo-tolerant matching and scans conventional
directories such as `tests/`, `examples/`, `scripts/`, and `cmd/`. Set
`--chaos 0` for declared commands only or `--chaos 2` for a broader bounded
scan. Broader discovery never permits arbitrary command generation.

### Supported project sources

Discovery is static: `dev` reads manifests and task files but does not invoke
native task-listing commands during detection.

| Category | Sources |
|---|---|
| Package ecosystems | npm, pnpm, Yarn, Bun, Vite, Next.js, Cargo, Composer, Artisan, Go, Gradle, Maven, .NET, SwiftPM, Dart, Flutter, Zig, and Sema |
| Project task facades | Just, Make, Jake, Taskfile, and mise |
| Services and runnable targets | Docker Compose, shell scripts, Python files, and PHP files |

Workspace-aware discovery covers Cargo workspaces, Node/pnpm workspaces, Go
workspaces, Gradle multi-project builds, Maven reactors, and .NET solutions.
Gradle and Maven wrappers are selected only when their declared distributions
are already cached locally, so discovery cannot trigger a download.

### Doctor and remembered choices

Check every registered local toolchain with bounded, tool-specific probes:

```console
dev doctor
```

Explicit picker choices can be remembered for the same project shape and
invocation. Inspect or clear them with:

```console
dev cache list
dev cache clear
```

## Shell completions

Generate completion scripts from the installed binary for Bash, Zsh, Fish,
Elvish, Nushell, or PowerShell:

```console
dev completions --install

# Or generate/install a specific shell explicitly:
dev completions zsh --install
dev completions nushell --install
dev completions bash > ~/.local/share/bash-completion/completions/dev
dev completions zsh > ~/.zsh/completions/_dev
dev completions fish > ~/.config/fish/completions/dev.fish
dev completions elvish > ~/.config/elvish/lib/dev.elv
dev completions nushell > ~/.config/nushell/completions/dev.nu
dev completions powershell >> $PROFILE
```

`--install` detects the current shell from `$SHELL`, respects the XDG and
`$ZDOTDIR` configuration directories, creates the user-local completion
directory, and writes the script there. It prints any shell-specific activation
step. PowerShell requires the manual `$PROFILE` command shown above. Restart the
shell after installing a completion script.

## Project principles

1. Discover commands; do not define a new task language.
2. Select only commands constructed from known project evidence.
3. Never install packages or access the network during discovery.
4. Make hints meaningful and ambiguity explicit.
5. Keep resolution deterministic and explainable.
6. Preserve the real command's process behavior exactly.

The complete contract and implementation milestones live in
[`docs/spec.md`](docs/spec.md).

## Contributing

Human-written and AI-assisted contributions are both welcome. Start with the
[contribution policy](CONTRIBUTING.md); detector authors should then follow the
step-by-step [detector guide](docs/adding-a-detector.md). Coding agents must also
read [`AGENTS.md`](AGENTS.md) before changing the repository.

## Development

The project targets Rust 1.85 or newer and uses
[`cargo-nextest`](https://nexte.st/) for its test suite.

```console
just test
just qa
just bench
```

`just qa` runs formatting checks, Clippy with warnings denied, and the nextest
suite. `just bench` builds the release binary and checks the performance
ceilings documented in [`docs/performance.md`](docs/performance.md). Doctests
are intentionally not part of the project.
