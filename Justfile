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

# Install the dev binary globally from this checkout.
[group('install')]
install:
    cargo install --path .

# Remove the globally installed dev binary package.
[group('install')]
uninstall:
    cargo uninstall dev-launcher

# Run the release-mode performance suite.
[group('qa')]
bench:
    cargo build --release --bin dev
    cargo bench --bench performance

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

# Edit website/og.html, never og.png. Override the browser with $CHROME.
# Render the Open Graph card to website/og.png (1200x630).
[group('web')]
og:
    @CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"; \
    test -x "$CHROME" || { echo "No Chrome at $CHROME - set \$CHROME" >&2; exit 1; }; \
    "$CHROME" --headless --disable-gpu --hide-scrollbars \
        --force-device-scale-factor=1 --window-size=1200,630 \
        --screenshot=website/og.png "file://$PWD/website/og.html"
