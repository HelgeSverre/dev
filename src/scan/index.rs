use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ignore::WalkBuilder;
use smallvec::SmallVec;

use super::manifest::DiscoveryFiles;
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
    pub manifests: DiscoveryFiles,
    pub truncated: Vec<Truncation>,
    pub(crate) structural_complete: bool,
}

impl FileIndex {
    pub fn build(roots: &RootInfo, options: ScanOptions) -> Self {
        let (structural, was_truncated) = collect_walk(
            &roots.scan_root,
            Some(options.structural_depth),
            options.hard_cap,
        );
        let structural_complete = options.structural_depth > 0
            && !was_truncated
            && !structural.iter().any(|entry| {
                entry.file_type == IndexedFileType::Directory
                    && entry.relative_path.components().count() == options.structural_depth
            });
        let mut index = Self {
            structural,
            structural_complete,
            manifests: roots.discovery_files.clone(),
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
        if relative_anchor.as_os_str().is_empty() {
            return;
        }
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
        let patterns = workspace_manifest_patterns(&roots.scan_root, &self.manifests);
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
        let structural = &self.structural;
        let mut structural_index = 0;
        self.targets.retain(|target| {
            while structural
                .get(structural_index)
                .is_some_and(|entry| stable_entry_cmp(entry, target).is_lt())
            {
                structural_index += 1;
            }
            structural
                .get(structural_index)
                .is_none_or(|entry| entry.relative_path != target.relative_path)
        });
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
            executable: executable(directory_entry.path(), &metadata),
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

fn workspace_manifest_patterns(root: &Path, files: &DiscoveryFiles) -> WorkspacePatterns {
    let mut output = WorkspacePatterns::default();
    for workspace in crate::registry::registrations()
        .iter()
        .filter_map(|registration| registration.workspace)
    {
        let contribution = workspace.scan_contribution(root, files);
        output.includes.extend(contribution.includes);
        output.excludes.extend(contribution.excludes);
    }
    output.includes.sort();
    output.includes.dedup();
    output.excludes.sort();
    output.excludes.dedup();
    output
}

fn compile_globs(patterns: &[String]) -> Result<globset::GlobSet, globset::Error> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern)?);
    }
    builder.build()
}

fn executable(path: &Path, metadata: &std::fs::Metadata) -> bool {
    crate::path::is_executable(path, metadata)
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

#[cfg(windows)]
fn stable_os_cmp(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> Ordering {
    use std::os::windows::ffi::OsStrExt as _;

    left.encode_wide().cmp(right.encode_wide())
}

#[cfg(not(any(unix, windows)))]
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
    fn go_work_static_use_directives_are_parsed_without_execution() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(
            temp.path().join("go.work"),
            "go 1.26\nuse ./apps/api\nuse (\n  ./deep/services/worker\n  \"tools/job\"\n)\n",
        )?;
        assert_eq!(
            crate::registry::workspace(crate::registry::GO)
                .expect("Go workspace contributor")
                .scan_contribution(temp.path(), &DiscoveryFiles::default())
                .includes,
            [
                "apps/api/go.mod",
                "deep/services/worker/go.mod",
                "tools/job/go.mod"
            ]
        );
        Ok(())
    }

    #[test]
    fn cargo_workspace_globs_include_default_members_and_respect_excludes() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"deep/crates/*\"]\ndefault-members = [\"deep/tools/*\"]\nexclude = [\"deep/crates/ignored\"]\n",
        )?;
        for member in [
            "deep/crates/app",
            "deep/crates/ignored",
            "deep/tools/release",
        ] {
            std::fs::create_dir_all(temp.path().join(member).join("src"))?;
            std::fs::write(
                temp.path().join(member).join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{}\"\nversion = \"0.1.0\"\n",
                    member.replace('/', "-")
                ),
            )?;
            std::fs::write(temp.path().join(member).join("src/main.rs"), "")?;
        }

        let roots = resolve_roots(&Target::Directory(temp.path().to_path_buf()));
        let index = FileIndex::build(
            &roots,
            ScanOptions {
                structural_depth: 1,
                hard_cap: 20_000,
            },
        );

        assert!(index
            .find_relative(Path::new("deep/crates/app/Cargo.toml"))
            .is_some());
        assert!(index
            .find_relative(Path::new("deep/tools/release/Cargo.toml"))
            .is_some());
        assert!(index
            .find_relative(Path::new("deep/crates/ignored/Cargo.toml"))
            .is_none());
        Ok(())
    }
}
