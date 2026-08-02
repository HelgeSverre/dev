use std::path::{Path, PathBuf};

use crate::intent::Target;
use crate::registry::{MarkerPattern, RootRole};

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
        .find(|directory| {
            has_registered_marker(directory, |role| {
                matches!(role, RootRole::Package | RootRole::Classified)
            })
        })
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
    let temporary_root = std::env::temp_dir();
    let mut ancestors = Vec::new();
    for directory in anchor.ancestors() {
        if directory == temporary_root && directory != anchor {
            break;
        }
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

fn has_registered_marker(directory: &Path, role: impl Fn(RootRole) -> bool) -> bool {
    let entries = std::fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    crate::registry::markers()
        .iter()
        .filter(|marker| role(marker.root_role))
        .any(|marker| match marker.pattern {
            MarkerPattern::Exact(name) => directory.join(name).is_file(),
            MarkerPattern::AsciiCaseInsensitiveBasename(_) | MarkerPattern::Extension(_) => {
                entries.iter().any(|path| marker.pattern.matches(path))
            }
        })
}

fn is_workspace(directory: &Path) -> bool {
    if has_registered_marker(directory, |role| role == RootRole::Workspace) {
        return true;
    }
    crate::registry::registrations()
        .iter()
        .filter_map(|registration| registration.workspace)
        .any(|workspace| workspace.is_workspace(directory))
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

    #[test]
    fn registered_task_and_extension_markers_drive_roots() -> anyhow::Result<()> {
        let task = tempfile::tempdir()?;
        std::fs::create_dir_all(task.path().join("nested"))?;
        std::fs::write(task.path().join("Taskfile.yml"), "tasks: {}\n")?;
        let task_roots = resolve_roots(&Target::Directory(task.path().join("nested")));
        assert_eq!(task_roots.package_root.as_deref(), Some(task.path()));

        let dotnet = tempfile::tempdir()?;
        std::fs::create_dir_all(dotnet.path().join("src/App"))?;
        std::fs::write(dotnet.path().join("App.sln"), "")?;
        std::fs::write(dotnet.path().join("src/App/App.csproj"), "<Project />")?;
        let dotnet_roots = resolve_roots(&Target::Directory(dotnet.path().join("src/App")));
        assert_eq!(
            dotnet_roots.package_root.as_deref(),
            Some(dotnet.path().join("src/App").as_path())
        );
        assert_eq!(dotnet_roots.workspace_root.as_deref(), Some(dotnet.path()));
        Ok(())
    }
}
