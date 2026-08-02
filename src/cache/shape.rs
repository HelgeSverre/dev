use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::cache::CacheError;
use crate::candidate::{Availability, Candidate};
use crate::intent::Target;
use crate::registry::MarkerPattern;
use crate::scan::{FileIndex, RootInfo};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShapeSnapshot {
    semantic_files: Vec<FileDigest>,
    watched_paths: Vec<PathMetadata>,
    #[serde(with = "super::serde_os::path")]
    logical_root: PathBuf,
    #[serde(with = "super::serde_os::option_path")]
    physical_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct FileDigest {
    #[serde(with = "super::serde_os::path")]
    path: PathBuf,
    digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct PathMetadata {
    #[serde(with = "super::serde_os::path")]
    path: PathBuf,
    exists: bool,
    directory: bool,
    size: u64,
    modified_nanos: Option<u128>,
}

impl ShapeSnapshot {
    pub fn capture(
        roots: &RootInfo,
        index: &FileIndex,
        candidate: &Candidate,
        target: &Target,
    ) -> Result<Self, CacheError> {
        let semantic_paths = index.manifests.read_paths()?;
        let semantic_files = semantic_paths
            .iter()
            .filter_map(|path| {
                digest_file(path).map(|digest| FileDigest {
                    path: path.clone(),
                    digest,
                })
            })
            .collect::<Vec<_>>();

        let mut watched = BTreeSet::from([
            roots.scan_root.clone(),
            roots.logical_anchor.clone(),
            candidate.cwd.clone(),
            target.path().to_path_buf(),
        ]);
        if let Some(physical_anchor) = &roots.physical_anchor {
            watched.insert(physical_anchor.clone());
        }
        watched.extend(semantic_paths.iter().cloned());
        watched.extend(
            semantic_paths
                .iter()
                .filter_map(|path| path.parent().map(Path::to_path_buf)),
        );
        watched.extend(candidate.evidence.iter().filter_map(|evidence| {
            evidence.source.as_ref().map(|source| {
                if source.is_absolute() {
                    source.clone()
                } else {
                    roots.scan_root.join(source)
                }
            })
        }));
        let marker_roots = [
            Some(roots.scan_root.clone()),
            roots.package_root.clone(),
            roots.workspace_root.clone(),
            Some(candidate.cwd.clone()),
        ];
        for root in marker_roots.into_iter().flatten() {
            watched.insert(root.join(".git"));
            watched.extend(registered_marker_paths(&root));
        }
        if let Availability::Available { resolved_program } = &candidate.availability {
            if resolved_program.starts_with(&roots.scan_root) {
                watched.insert(resolved_program.clone());
            }
        }
        let watched_paths = watched.into_iter().map(PathMetadata::capture).collect();
        Ok(Self {
            semantic_files,
            watched_paths,
            logical_root: roots.scan_root.clone(),
            physical_root: roots.scan_root.canonicalize().ok(),
        })
    }

    #[must_use]
    pub fn is_current(&self) -> bool {
        if self.logical_root.canonicalize().ok() != self.physical_root {
            return false;
        }
        self.semantic_files
            .iter()
            .all(|file| digest_file(&file.path).as_deref() == Some(file.digest.as_str()))
            && self
                .watched_paths
                .iter()
                .all(|metadata| PathMetadata::capture(metadata.path.clone()) == *metadata)
    }
}

fn registered_marker_paths(root: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    let mut paths = crate::registry::markers()
        .iter()
        .flat_map(|marker| match marker.pattern {
            MarkerPattern::Exact(name) => vec![root.join(name)],
            MarkerPattern::AsciiCaseInsensitiveBasename(_)
            | MarkerPattern::BasenamePrefixSuffix { .. }
            | MarkerPattern::Extension(_) => entries
                .iter()
                .filter(|path| marker.pattern.matches(path))
                .cloned()
                .collect(),
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

impl PathMetadata {
    fn capture(path: PathBuf) -> Self {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => Self {
                path,
                exists: true,
                directory: metadata.is_dir(),
                size: metadata.len(),
                modified_nanos: metadata
                    .modified()
                    .ok()
                    .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos()),
            },
            Err(_) => Self {
                path,
                exists: false,
                directory: false,
                size: 0,
                modified_nanos: None,
            },
        }
    }
}

fn digest_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(blake3::hash(&bytes).to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use crate::candidate::{Candidate, SelectionPolicy};
    use crate::intent::{Intent, Target};
    use crate::registry::{NODE, NODE_SOURCE};
    use crate::scan::{resolve_roots, FileIndex, ScanOptions};

    use super::*;

    fn candidate(cwd: &Path) -> Candidate {
        Candidate::new(
            "test:run",
            NODE,
            NODE_SOURCE,
            Intent::Run,
            "run",
            "missing-test-program",
            Vec::new(),
            cwd.to_path_buf(),
            80,
            SelectionPolicy::Automatic,
        )
    }

    #[test]
    fn semantic_manifest_content_changes_invalidate_snapshot() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let manifest = temp.path().join("package.json");
        std::fs::write(&manifest, r#"{"scripts":{"dev":"vite"}}"#)?;
        let target = Target::Directory(temp.path().to_path_buf());
        let roots = resolve_roots(&target);
        let index = FileIndex::build(&roots, ScanOptions::default());
        index.manifests.read(&manifest)?;
        let snapshot = ShapeSnapshot::capture(&roots, &index, &candidate(temp.path()), &target)?;
        assert!(snapshot.is_current());

        std::fs::write(&manifest, r#"{"scripts":{"dev":"next"}}"#)?;
        assert!(!snapshot.is_current());
        Ok(())
    }

    #[test]
    fn adding_a_root_config_invalidates_watched_missing_path() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::create_dir(temp.path().join(".git"))?;
        let target = Target::Directory(temp.path().to_path_buf());
        let roots = resolve_roots(&target);
        let index = FileIndex::build(&roots, ScanOptions::default());
        let snapshot = ShapeSnapshot::capture(&roots, &index, &candidate(temp.path()), &target)?;
        assert!(snapshot.is_current());

        std::fs::write(temp.path().join("package.json"), "{}")?;
        assert!(!snapshot.is_current());
        Ok(())
    }

    #[test]
    fn adding_registered_extension_marker_invalidates_snapshot() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::create_dir(temp.path().join(".git"))?;
        let target = Target::Directory(temp.path().to_path_buf());
        let roots = resolve_roots(&target);
        let index = FileIndex::build(&roots, ScanOptions::default());
        let snapshot = ShapeSnapshot::capture(&roots, &index, &candidate(temp.path()), &target)?;
        assert!(snapshot.is_current());

        std::fs::write(temp.path().join("App.csproj"), "<Project />")?;
        assert!(!snapshot.is_current());
        Ok(())
    }
}
