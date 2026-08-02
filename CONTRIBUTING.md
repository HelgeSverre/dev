# Contributing to dev

Thanks for helping improve `dev`. Contributions of all sizes are welcome, from
documentation fixes and detector fixtures to new ecosystems and execution
correctness work.

## Ground rules

- **No CLA or DCO sign-off is required.** By opening a pull request, you agree
  that your contribution may be distributed under the project's MIT OR
  Apache-2.0 license terms.
- **Be respectful.** Review the work, not the person, and assume good intent.
- **Discuss large changes first.** Open an issue before investing in a new
  subsystem, broad refactor, dependency, or behavioral change so the approach
  can be agreed before implementation.
- **Treat the specification as normative.** If implementation and
  [`docs/spec.md`](docs/spec.md) disagree, either fix the implementation or
  propose an explicit specification change. Do not quietly weaken a safety or
  resolution invariant to make a test pass.

## AI-assisted contributions are welcome

`dev` is intentionally friendly to coding agents. [`AGENTS.md`](AGENTS.md)
contains the canonical repository constraints, architecture invariants, and
verification standards. Agents implementing a detector should also read
[`docs/adding-a-detector.md`](docs/adding-a-detector.md).

AI assistance should raise the quality of a contribution, not increase its
volume. We ask that:

- **You understand and stand behind every submitted line.** You are the author,
  not the tool. Be ready to explain the behavior and revise it in review.
- **The change actually runs.** Build, nextest, formatting, Clippy, and any
  relevant benchmark or end-to-end checks must pass locally. Generated claims
  are not verification.
- **The change is scoped and genuine.** Mass-generated refactors, speculative
  abstractions, drive-by reformatting, and unreviewed generated changes may be
  closed without detailed review.
- **Evidence is described honestly.** A parser unit test is not an end-to-end
  CLI test, a fixture is not a real project, and a debug build is not a
  performance benchmark.
- **Existing work is preserved.** Inspect the working tree before editing and
  do not overwrite unrelated changes.

Tool disclosure is not required. AI-assisted and hand-written contributions
are held to the same standard.

## Development workflow

The project requires Rust 1.85 or newer, `just`, and `cargo-nextest`.

```console
just test       # run unit and integration tests with nextest
just detector-check # enforce detector registration and discovery invariants
just qa         # formatting check + Clippy -D warnings + nextest
just bench      # release-mode performance suite
```

Before opening a pull request:

1. Add tests at the layer the change affects. User-visible CLI behavior needs
   an integration test, not only a helper-unit test.
2. Run `just qa` and confirm it passes.
3. Run `just bench` when changing scanning, matching, ranking, registry lookup,
   caching, startup, or another measured path.
4. Inspect every golden-file change and explain the intended semantic
   difference. Never regenerate a golden merely to make a failure disappear.
5. Update the README or specification when behavior visible to users or
   detector authors changes.
6. Keep the pull request focused on one coherent vertical slice and describe
   both what changed and why.

CI repeats formatting and Clippy checks, runs nextest on Linux, macOS, and
Windows, validates the Rust 1.85 minimum supported version, and exposes the
detector contract and release performance ceilings as separately named checks.

## Adding or changing a detector

Follow [Adding a detector](docs/adding-a-detector.md). The central rule is that
a detector is a deterministic, read-only projection of project evidence into
known argv—not a hook for running an ecosystem's task-list command.

A detector contribution normally includes:

- the detector implementation and its single capability-registry entry;
- bounded parsing and actionable diagnostics for malformed input;
- positive, negative, ambiguity, and availability tests;
- a corpus fixture and reviewed golden change when structural discovery is
  affected;
- CLI-level `--why` or JSON coverage for user-visible resolution behavior; and
- specification or README updates for newly supported behavior.

## Repository map

- `src/detect/` — static ecosystem detectors and candidate construction.
- `src/registry.rs` — detector, source, tool, marker, workspace, target, doctor,
  and cache metadata.
- `src/scan/` — root resolution and bounded file/manifest indexing.
- `src/query/`, `src/resolve.rs`, `src/score.rs` — matching and deterministic
  resolution.
- `src/cache/` — remembered choices and project-shape invalidation.
- `src/exec/` — process semantics and platform-specific execution.
- `tests/fixtures/corpus/` — small structural discovery repositories.
- `tests/cli_test.rs` — built-binary integration coverage.
- `docs/spec.md` — normative product contract.

## Questions

Open an issue on the repository. Include a minimal project shape, the command
you ran, and `dev ... --why` or `--json` output when reporting discovery or
ranking behavior. Remove secrets and private paths before posting diagnostics.
