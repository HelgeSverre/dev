# Detection fixture corpus

These repositories are deliberately tiny but retain the files and nesting that
drive real command discovery. `corpus_golden_test` snapshots all three intents
for every numbered repository.

Symlink and non-UTF-8 cases are created at runtime in `cli_test.rs`; committing
those names would make the corpus impossible to check out on every supported
filesystem.
