use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ignore::WalkBuilder;
use smallvec::SmallVec;

use super::manifest::ManifestCache;
use super::RootInfo;

pub type EntryId = usize;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IndexedFileType {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Debug)]
pub struct IndexEntry {
    pub relative_path: PathBuf,
    pub file_type: IndexedFileType,
    pub executable: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl IndexEntry {
    #[must_use]
    pub fn absolute_path(&self, root: &Path) -> PathBuf {
        root.join(&self.relative_path)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Truncation {
    pub scope: PathBuf,
    pub limit: usize,
}

#[derive(Copy, Clone, Debug)]
pub struct ScanOptions {
    pub structural_depth: usize,
    pub hard_cap: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            structural_depth: 3,
            hard_cap: 20_000,
        }
    }
}

#[derive(Debug, Default)]
pub struct FileIndex {
    pub structural: Vec<IndexEntry>,
    pub targets: Vec<IndexEntry>,
    pub by_name: HashMap<OsString, SmallVec<[EntryId; 2]>>,
    pub by_extension: HashMap<OsString, SmallVec<[EntryId; 8]>>,
    pub manifests: ManifestCache,
    pub truncated: Vec<Truncation>,
}

impl FileIndex {
    pub fn build(roots: &RootInfo, options: ScanOptions) -> Self {
        let (structural, was_truncated) = collect_walk(
            &roots.scan_root,
            Some(options.structural_depth),
            options.hard_cap,
        );
        let mut index = Self {
            structural,
            ..Self::default()
        };
        index.include_declared_workspace_manifests(roots, options.hard_cap);
        index.include_anchor_neighborhood(roots, options.hard_cap);
        index.structural.sort_by(stable_entry_cmp);
        index
            .structural
            .dedup_by(|left, right| left.relative_path == right.relative_path);
        if index.structural.len() > options.hard_cap {
            index.structural.truncate(options.hard_cap);
            index.truncated.push(Truncation {
                scope: roots.scan_root.clone(),
                limit: options.hard_cap,
            });
        } else if was_truncated {
            index.truncated.push(Truncation {
                scope: roots.scan_root.clone(),
                limit: options.hard_cap,
            });
        }
        index.rebuild_lookup();
        index
    }

    fn include_anchor_neighborhood(&mut self, roots: &RootInfo, hard_cap: usize) {
        let Ok(relative_anchor) = roots.logical_anchor.strip_prefix(&roots.scan_root) else {
            return;
        };
        let (entries, truncated) = collect_walk(&roots.logical_anchor, Some(1), hard_cap);
        self.structural.extend(entries.into_iter().map(|mut entry| {
            entry.relative_path = relative_anchor.join(entry.relative_path);
            entry
        }));
        if truncated {
            self.truncated.push(Truncation {
                scope: roots.logical_anchor.clone(),
                limit: hard_cap,
            });
        }
    }

    fn include_declared_workspace_manifests(&mut self, roots: &RootInfo, hard_cap: usize) {
        let patterns = workspace_manifest_patterns(&roots.scan_root);
        if patterns.includes.is_empty() {
            return;
        }
        let Ok(includes) = compile_globs(&patterns.includes) else {
            return;
        };
        let excludes = compile_globs(&patterns.excludes).ok();
        let (entries, truncated) = collect_walk(&roots.scan_root, None, hard_cap);
        let declared_manifests = entries
            .iter()
            .filter(|entry| {
                entry.file_type == IndexedFileType::File
                    && includes.is_match(&entry.relative_path)
                    && !excludes
                        .as_ref()
                        .is_some_and(|patterns| patterns.is_match(&entry.relative_path))
            })
            .map(|entry| entry.relative_path.clone())
            .collect::<Vec<_>>();
        let member_directories = declared_manifests
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        self.structural.extend(entries.into_iter().filter(|entry| {
            declared_manifests.contains(&entry.relative_path)
                || member_directories.iter().any(|directory| {
                    entry
                        .relative_path
                        .strip_prefix(directory)
                        .is_ok_and(|relative| relative.components().count() <= 3)
                })
        }));
        if truncated {
            self.truncated.push(Truncation {
                scope: roots.scan_root.clone(),
                limit: hard_cap,
            });
        }
    }

    pub(crate) fn append_targets(
        &mut self,
        entries: Vec<IndexEntry>,
        truncation: Option<Truncation>,
    ) {
        self.targets.extend(entries);
        self.targets.sort_by(stable_entry_cmp);
        self.targets
            .dedup_by(|left, right| left.relative_path == right.relative_path);
        if let Some(truncation) = truncation {
            self.truncated.push(truncation);
        }
        self.rebuild_lookup();
    }

    fn rebuild_lookup(&mut self) {
        self.by_name.clear();
        self.by_extension.clear();
        for (id, entry) in self.structural.iter().chain(&self.targets).enumerate() {
            if let Some(name) = entry.relative_path.file_name() {
                self.by_name
                    .entry(name.to_os_string())
                    .or_default()
                    .push(id);
            }
            if let Some(extension) = entry.relative_path.extension() {
                self.by_extension
                    .entry(extension.to_os_string())
                    .or_default()
                    .push(id);
            }
        }
    }

    pub fn all_entries(&self) -> impl Iterator<Item = &IndexEntry> {
        self.structural.iter().chain(&self.targets)
    }

    #[must_use]
    pub fn find_relative(&self, relative_path: &Path) -> Option<&IndexEntry> {
        self.all_entries()
            .find(|entry| entry.relative_path == relative_path)
    }
}

pub(crate) fn collect_walk(
    root: &Path,
    max_depth: Option<usize>,
    hard_cap: usize,
) -> (Vec<IndexEntry>, bool) {
    let mut builder = WalkBuilder::new(root);
    builder
        .max_depth(max_depth)
        .follow_links(false)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .filter_entry(|entry| !is_default_ignored(entry.path()))
        .sort_by_file_path(stable_path_cmp);

    let mut entries = Vec::new();
    let mut truncated = false;
    for result in builder.build() {
        let Ok(directory_entry) = result else {
            continue;
        };
        if directory_entry.path() == root {
            continue;
        }
        if entries.len() == hard_cap {
            truncated = true;
            break;
        }
        let Ok(relative_path) = directory_entry.path().strip_prefix(root) else {
            continue;
        };
        let Ok(metadata) = std::fs::symlink_metadata(directory_entry.path()) else {
            continue;
        };
        let file_type = if metadata.file_type().is_symlink() {
            IndexedFileType::Symlink
        } else if metadata.is_dir() {
            IndexedFileType::Directory
        } else if metadata.is_file() {
            IndexedFileType::File
        } else {
            continue;
        };
        entries.push(IndexEntry {
            relative_path: relative_path.to_path_buf(),
            file_type,
            executable: executable(&metadata),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }
    (entries, truncated)
}

fn is_default_ignored(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        matches!(
            name.to_str(),
            Some(
                ".git"
                    | "node_modules"
                    | "vendor"
                    | "target"
                    | "dist"
                    | "build"
                    | ".cache"
                    | ".next"
                    | ".turbo"
                    | ".venv"
                    | "__pycache__"
            )
        )
    })
}

#[derive(Default)]
struct WorkspacePatterns {
    includes: Vec<String>,
    excludes: Vec<String>,
}

fn workspace_manifest_patterns(root: &Path) -> WorkspacePatterns {
    let mut output = WorkspacePatterns::default();
    if let Ok(contents) = std::fs::read_to_string(root.join("Cargo.toml")) {
        if let Ok(manifest) = toml::from_str::<toml::Value>(&contents) {
            if let Some(workspace) = manifest.get("workspace") {
                output.includes.extend(
                    toml_strings(workspace.get("members"))
                        .map(|pattern| append_manifest(pattern, "Cargo.toml")),
                );
                output.excludes.extend(
                    toml_strings(workspace.get("exclude"))
                        .map(|pattern| append_manifest(pattern, "Cargo.toml")),
                );
            }
        }
    }
    if let Ok(contents) = std::fs::read_to_string(root.join("package.json")) {
        if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&contents) {
            let workspaces = manifest.get("workspaces");
            let values = workspaces
                .and_then(serde_json::Value::as_array)
                .or_else(|| workspaces?.get("packages")?.as_array());
            if let Some(values) = values {
                output.includes.extend(values.iter().filter_map(|value| {
                    value
                        .as_str()
                        .map(|pattern| append_manifest(pattern, "package.json"))
                }));
            }
        }
    }
    if let Ok(contents) = std::fs::read_to_string(root.join("pnpm-workspace.yaml")) {
        if let Ok(manifest) = serde_yaml::from_str::<serde_yaml::Value>(&contents) {
            if let Some(packages) = manifest
                .get("packages")
                .and_then(serde_yaml::Value::as_sequence)
            {
                for pattern in packages.iter().filter_map(serde_yaml::Value::as_str) {
                    if let Some(excluded) = pattern.strip_prefix('!') {
                        output
                            .excludes
                            .push(append_manifest(excluded, "package.json"));
                    } else {
                        output
                            .includes
                            .push(append_manifest(pattern, "package.json"));
                    }
                }
            }
        }
    }
    if let Ok(contents) = std::fs::read_to_string(root.join("go.work")) {
        output.includes.extend(
            go_work_uses(&contents)
                .into_iter()
                .map(|directory| append_manifest(&directory, "go.mod")),
        );
    }
    output.includes.sort();
    output.includes.dedup();
    output.excludes.sort();
    output.excludes.dedup();
    output
}

fn go_work_uses(contents: &str) -> Vec<String> {
    let mut directories = Vec::new();
    let mut in_block = false;
    for source_line in contents.lines() {
        let line = source_line
            .split_once("//")
            .map_or(source_line, |(before, _)| before)
            .trim();
        if line.is_empty() {
            continue;
        }
        if in_block {
            if line == ")" {
                in_block = false;
            } else if let Some(directory) = static_go_work_path(line) {
                directories.push(directory);
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("use") else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim();
        if rest == "(" {
            in_block = true;
        } else if let Some(directory) = static_go_work_path(rest) {
            directories.push(directory);
        }
    }
    directories.sort();
    directories.dedup();
    directories
}

fn static_go_work_path(value: &str) -> Option<String> {
    let value = value
        .split_whitespace()
        .next()?
        .trim_matches(['"', '\u{60}'])
        .trim_start_matches("./");
    (!value.is_empty()
        && !Path::new(value).is_absolute()
        && !Path::new(value)
            .components()
            .any(|component| component == std::path::Component::ParentDir))
    .then(|| value.to_owned())
}

fn toml_strings(value: Option<&toml::Value>) -> impl Iterator<Item = &str> {
    value
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
}

fn append_manifest(pattern: &str, manifest: &str) -> String {
    format!("{}/{manifest}", pattern.trim_end_matches(['/', '\\']))
}

fn compile_globs(patterns: &[String]) -> Result<globset::GlobSet, globset::Error> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern)?);
    }
    builder.build()
}

#[cfg(unix)]
fn executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn stable_entry_cmp(left: &IndexEntry, right: &IndexEntry) -> Ordering {
    stable_path_cmp(&left.relative_path, &right.relative_path)
}

fn stable_path_cmp(left: &Path, right: &Path) -> Ordering {
    stable_os_cmp(left.as_os_str(), right.as_os_str())
}

#[cfg(unix)]
fn stable_os_cmp(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> Ordering {
    use std::os::unix::ffi::OsStrExt;
    left.as_bytes().cmp(right.as_bytes())
}

#[cfg(not(unix))]
fn stable_os_cmp(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> Ordering {
    left.to_string_lossy().cmp(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use crate::intent::Target;
    use crate::scan::resolve_roots;

    use super::*;

    #[test]
    fn scan_order_is_stable_and_default_outputs_are_ignored() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::create_dir_all(temp.path().join("src"))?;
        std::fs::create_dir_all(temp.path().join("target/debug"))?;
        std::fs::write(temp.path().join("src/z.rs"), "")?;
        std::fs::write(temp.path().join("src/a.rs"), "")?;
        std::fs::write(temp.path().join("target/debug/app"), "")?;
        let roots = resolve_roots(&Target::Directory(temp.path().to_path_buf()));
        let index = FileIndex::build(&roots, ScanOptions::default());
        let paths = index
            .structural
            .iter()
            .map(|entry| entry.relative_path.as_path())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                Path::new("src"),
                Path::new("src/a.rs"),
                Path::new("src/z.rs")
            ]
        );
        Ok(())
    }

    #[test]
    fn go_work_static_use_directives_are_parsed_without_execution() {
        assert_eq!(
            go_work_uses(
                "go 1.26\nuse ./apps/api\nuse (\n  ./deep/services/worker\n  \"tools/job\"\n)\n"
            ),
            ["apps/api", "deep/services/worker", "tools/job"]
        );
    }
}
