# dev — zero-setup project command launcher

# Show available recipes.
default:
    @just --list --unsorted

# Build the debug binary.
[group('build')]
build:
    cargo build

# Build the release binary.
[group('build')]
release:
    cargo build --release

# Run dev from source, passing arguments through.
[group('build')]
run *ARGS:
    cargo run -- {{ ARGS }}

# Run unit and integration tests with nextest.
[group('qa')]
test *ARGS:
    cargo nextest run {{ ARGS }}

# Format Rust sources.
[group('qa')]
fmt:
    cargo fmt --all

# Verify formatting without modifying files.
[group('qa')]
fmt-check:
    cargo fmt --all -- --check

# Lint every target and deny warnings.
[group('qa')]
clippy:
    cargo clippy --all-targets -- -D warnings

# Type-check every target.
[group('qa')]
check:
    cargo check --all-targets

# Full non-mutating quality gate.
[group('qa')]
qa: fmt-check clippy test

# Remove Cargo build artifacts.
[group('build')]
clean:
    cargo clean
