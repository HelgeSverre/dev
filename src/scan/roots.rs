use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

use crate::intent::Target;

const PACKAGE_MARKERS: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "composer.json",
    "go.mod",
    "build.zig",
    "Package.swift",
    "pubspec.yaml",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootInfo {
    pub logical_anchor: PathBuf,
    pub physical_anchor: Option<PathBuf>,
    pub package_root: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
    pub repository_root: Option<PathBuf>,
    pub scan_root: PathBuf,
}

/// Resolve package, workspace, and scan roots without executing project code.
#[must_use]
pub fn resolve_roots(target: &Target) -> RootInfo {
    let logical_anchor = target.anchor_directory().to_path_buf();
    let physical_anchor = target.path().canonicalize().ok();
    let ancestors = bounded_ancestors(&logical_anchor);

    let package_root = ancestors
        .iter()
        .find(|directory| has_any_marker(directory, PACKAGE_MARKERS))
        .cloned();
    let workspace_root = ancestors
        .iter()
        .find(|directory| is_workspace(directory))
        .cloned();
    let repository_root = ancestors
        .iter()
        .find(|directory| directory.join(".git").exists())
        .cloned();
    let scan_root = workspace_root
        .clone()
        .or_else(|| package_root.clone())
        .or_else(|| repository_root.clone())
        .unwrap_or_else(|| logical_anchor.clone());

    RootInfo {
        logical_anchor,
        physical_anchor,
        package_root,
        workspace_root,
        repository_root,
        scan_root,
    }
}

fn bounded_ancestors(anchor: &Path) -> Vec<PathBuf> {
    let home = directories::BaseDirs::new().map(|directories| directories.home_dir().to_path_buf());
    let mut ancestors = Vec::new();
    for directory in anchor.ancestors() {
        ancestors.push(directory.to_path_buf());
        if directory.join(".git").exists()
            || home.as_deref() == Some(directory)
            || directory.parent().is_none()
        {
            break;
        }
    }
    ancestors
}

fn has_any_marker(directory: &Path, markers: &[&str]) -> bool {
    markers
        .iter()
        .any(|marker| directory.join(marker).is_file())
}

fn is_workspace(directory: &Path) -> bool {
    if directory.join("pnpm-workspace.yaml").is_file() || directory.join("go.work").is_file() {
        return true;
    }
    if let Ok(contents) = std::fs::read_to_string(directory.join("package.json")) {
        if serde_json::from_str::<JsonValue>(&contents)
            .ok()
            .is_some_and(|manifest| manifest.get("workspaces").is_some())
        {
            return true;
        }
    }
    std::fs::read_to_string(directory.join("Cargo.toml"))
        .ok()
        .and_then(|contents| toml::from_str::<TomlValue>(&contents).ok())
        .is_some_and(|manifest| manifest.get("workspace").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_member_does_not_hide_workspace() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let member = temp.path().join("apps/web/src");
        std::fs::create_dir_all(&member)?;
        std::fs::write(
            temp.path().join("pnpm-workspace.yaml"),
            "packages:\n  - apps/*\n",
        )?;
        std::fs::write(temp.path().join("apps/web/package.json"), "{}")?;
        let roots = resolve_roots(&Target::Directory(member));
        assert_eq!(roots.package_root, Some(temp.path().join("apps/web")));
        assert_eq!(roots.workspace_root.as_deref(), Some(temp.path()));
        assert_eq!(roots.scan_root, temp.path());
        Ok(())
    }
}
