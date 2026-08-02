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

> **Status:** Early development. The CLI described below is the intended
> interface and is not ready for general use yet.

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

## How it is designed to work

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

## Intended interface

```text
dev <run|build|test> [target] [hint ...] [-- passthrough ...]
```

Targets must be path-like, such as `./apps/web` or `../service`. Other words
are retrieval hints. This keeps parsing stable regardless of which files happen
to exist in the current directory.

Useful inspection modes will include:

```console
dev run --list
dev run vite frontend --why
dev test --json
dev build --dry-run -- --release
```

## Project principles

1. Discover commands; do not define a new task language.
2. Select only commands constructed from known project evidence.
3. Never install packages or access the network during discovery.
4. Make hints meaningful and ambiguity explicit.
5. Keep resolution deterministic and explainable.
6. Preserve the real command's process behavior exactly.

The complete contract and implementation milestones live in
[`docs/spec.md`](docs/spec.md).

## Development

The project targets Rust 1.85 or newer. It is being built as tested vertical
slices across parsing, discovery, detection, ranking, resolution, interaction,
caching, and execution.
