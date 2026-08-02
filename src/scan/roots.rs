use std::path::{Path, PathBuf};

use crate::intent::Target;
use crate::registry::{MarkerPattern, RootClassification, RootRole};
use crate::scan::manifest::DiscoveryFiles;

#[derive(Clone, Debug)]
pub struct RootInfo {
    pub logical_anchor: PathBuf,
    pub physical_anchor: Option<PathBuf>,
    pub package_root: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
    pub repository_root: Option<PathBuf>,
    pub scan_root: PathBuf,
    pub discovery_files: DiscoveryFiles,
}

impl PartialEq for RootInfo {
    fn eq(&self, other: &Self) -> bool {
        self.logical_anchor == other.logical_anchor
            && self.physical_anchor == other.physical_anchor
            && self.package_root == other.package_root
            && self.workspace_root == other.workspace_root
            && self.repository_root == other.repository_root
            && self.scan_root == other.scan_root
    }
}

impl Eq for RootInfo {}

/// Resolve package, workspace, and scan roots without executing project code.
#[must_use]
pub fn resolve_roots(target: &Target) -> RootInfo {
    let discovery_files = DiscoveryFiles::default();
    let logical_anchor = target.anchor_directory().to_path_buf();
    let physical_anchor = target.path().canonicalize().ok();
    let ancestors = bounded_ancestors(&logical_anchor);
    let classifications = ancestors
        .iter()
        .map(|directory| classify_directory(directory, &discovery_files))
        .collect::<Vec<_>>();

    let package_root = ancestors
        .iter()
        .zip(&classifications)
        .find(|(_, classification)| classification.package)
        .map(|(directory, _)| directory)
        .cloned();
    let workspace_root = ancestors
        .iter()
        .zip(&classifications)
        .find(|(_, classification)| classification.workspace)
        .map(|(directory, _)| directory)
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
        discovery_files,
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

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
struct DirectoryClassification {
    package: bool,
    workspace: bool,
}

fn classify_directory(directory: &Path, files: &DiscoveryFiles) -> DirectoryClassification {
    let entries = std::fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    let mut classification = DirectoryClassification::default();
    for registration in crate::registry::registrations() {
        for marker in registration.markers {
            let matches = match marker.pattern {
                MarkerPattern::Exact(name) => {
                    let path = directory.join(name);
                    path.is_file()
                        .then_some(path)
                        .into_iter()
                        .collect::<Vec<_>>()
                }
                MarkerPattern::AsciiCaseInsensitiveBasename(_)
                | MarkerPattern::BasenamePrefixSuffix { .. }
                | MarkerPattern::Extension(_) => entries
                    .iter()
                    .filter(|path| path.is_file() && marker.pattern.matches(path))
                    .cloned()
                    .collect(),
            };
            for marker_path in matches {
                match marker.root_role {
                    RootRole::Package => classification.package = true,
                    RootRole::Workspace => classification.workspace = true,
                    RootRole::Classified => {
                        let classified = registration
                            .workspace
                            .map_or(RootClassification::Neither, |workspace| {
                                workspace.classify_root(&marker_path, files)
                            });
                        classification.package |= classified.is_package();
                        classification.workspace |= classified.is_workspace();
                    }
                    RootRole::Auxiliary => {}
                }
            }
        }
    }
    classification
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

        let mise = tempfile::tempdir()?;
        std::fs::create_dir_all(mise.path().join("nested"))?;
        std::fs::write(
            mise.path().join("mise.local.toml"),
            "[tasks.test]\nrun = 'true'\n",
        )?;
        let mise_roots = resolve_roots(&Target::Directory(mise.path().join("nested")));
        assert_eq!(mise_roots.package_root.as_deref(), Some(mise.path()));

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

    #[test]
    fn classified_workspace_reads_are_shared_with_the_file_index() -> anyhow::Result<()> {
        let cargo = tempfile::tempdir()?;
        std::fs::create_dir_all(cargo.path().join("crates/app/src"))?;
        std::fs::write(
            cargo.path().join("Cargo.toml"),
            "[workspace]\nmembers = ['crates/app']\n",
        )?;
        std::fs::write(
            cargo.path().join("crates/app/Cargo.toml"),
            "[package]\nname = 'app'\nversion = '0.1.0'\n",
        )?;
        let roots = resolve_roots(&Target::Directory(cargo.path().join("crates/app/src")));
        assert_eq!(
            roots.package_root.as_deref(),
            Some(cargo.path().join("crates/app").as_path())
        );
        assert_eq!(roots.workspace_root.as_deref(), Some(cargo.path()));
        assert!(roots
            .discovery_files
            .read_paths()?
            .contains(&cargo.path().join("Cargo.toml")));

        let index = crate::scan::FileIndex::build(&roots, crate::scan::ScanOptions::default());
        assert!(index
            .manifests
            .read_paths()?
            .contains(&cargo.path().join("Cargo.toml")));
        Ok(())
    }

    #[test]
    fn maven_modules_classify_the_reactor_as_a_workspace() -> anyhow::Result<()> {
        let maven = tempfile::tempdir()?;
        std::fs::create_dir_all(maven.path().join("app/src"))?;
        std::fs::write(
            maven.path().join("pom.xml"),
            "<project><modules><module>app</module></modules></project>",
        )?;
        std::fs::write(maven.path().join("app/pom.xml"), "<project />")?;
        let roots = resolve_roots(&Target::Directory(maven.path().join("app/src")));
        assert_eq!(
            roots.package_root.as_deref(),
            Some(maven.path().join("app").as_path())
        );
        assert_eq!(roots.workspace_root.as_deref(), Some(maven.path()));
        Ok(())
    }
}
