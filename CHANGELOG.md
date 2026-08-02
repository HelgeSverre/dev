# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] — 2026-08-03

### Initial Release

Zero-setup command discovery and launcher for software projects. Point `dev` at a
directory or file, state an intent (`run`/`build`/`test`), and it discovers,
ranks, and executes the commands the project already knows how to run.

### Supported Ecosystems

**Package ecosystems:** npm, pnpm, Yarn, Bun, Vite, Next.js, SvelteKit, Cargo,
Composer, Artisan, Go, Gradle, Maven, .NET, SwiftPM, Dart, Flutter, Zig, Sema,
Python (pyproject.toml/uv/pytest), ReScript, CMake, Nim, and Odin

**Project task facades:** Just, Make, Jake, Taskfile, and mise

**Services and containers:** Docker Compose

**Standalone runnable targets:** Shell scripts, Python files, PHP files, Wren
files, and Lira files

### Features

- Deterministic command discovery across 30+ ecosystems
- Fuzzy matching with typo-tolerant retrieval
- Automatic selection of clear winners; interactive picker for ambiguity
- Explainable decisions via `--why` and `--json`
- Exact process semantics: argv, environment, working directory, I/O, signals, exit status
- Remembered choices per project shape with cache invalidation
- `dev doctor` for toolchain health checks
- `dev completions` for Bash, Zsh, Fish, Elvish, Nushell, and PowerShell
- Workspace-aware: Cargo, Node/pnpm, Go, Gradle, Maven, and .NET workspaces

[0.1.0]: https://github.com/HelgeSverre/dev/releases/tag/v0.1.0
