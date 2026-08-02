use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::cache::CacheError;
use crate::candidate::{Availability, Candidate};
use crate::intent::Target;
use crate::scan::{FileIndex, RootInfo};

const WATCHED_PROJECT_NAMES: &[&str] = &[
    ".git",
    "package.json",
    "pnpm-workspace.yaml",
    "pnpm-lock.yaml",
    "yarn.lock",
    "package-lock.json",
    "bun.lock",
    "bun.lockb",
    "Cargo.toml",
    "Cargo.lock",
    "composer.json",
    "composer.lock",
    "go.mod",
    "go.sum",
    "go.work",
    "build.zig",
    "Package.swift",
    "Package.resolved",
    "pubspec.yaml",
    "pubspec.lock",
    "Makefile",
    "makefile",
    "GNUmakefile",
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
    "Dockerfile",
];

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
        let marker_roots = [
            Some(roots.scan_root.clone()),
            roots.package_root.clone(),
            roots.workspace_root.clone(),
            Some(candidate.cwd.clone()),
        ];
        for root in marker_roots.into_iter().flatten() {
            watched.extend(WATCHED_PROJECT_NAMES.iter().map(|name| root.join(name)));
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
    use crate::scan::{resolve_roots, FileIndex, ScanOptions};

    use super::*;

    fn candidate(cwd: &Path) -> Candidate {
        Candidate::new(
            "test:run",
            "node",
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
}
