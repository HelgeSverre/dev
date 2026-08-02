use std::collections::BTreeSet;
use std::path::PathBuf;

use super::index::{collect_walk, Truncation};
use super::{FileIndex, RootInfo};

const CONVENTIONAL_ROOTS: &[&str] = &[
    "bin", "cmd", "examples", "scripts", "spec", "test", "tests", "tools",
];

impl FileIndex {
    /// Add the wider target index selected by the invocation's chaos level.
    pub fn build_targets(&mut self, roots: &RootInfo, chaos: u8, hard_cap: usize) {
        if chaos == 0 {
            return;
        }

        let mut entries = Vec::new();
        let mut truncation = None;
        if chaos == 1 {
            for (relative_root, root) in self.conventional_roots(roots) {
                let remaining = hard_cap.saturating_sub(entries.len());
                if remaining == 0 {
                    truncation = Some(Truncation {
                        scope: roots.scan_root.clone(),
                        limit: hard_cap,
                    });
                    break;
                }
                let (found, truncated) = collect_walk(&root, None, remaining);
                entries.extend(found.into_iter().map(|mut entry| {
                    entry.relative_path = relative_root.join(entry.relative_path);
                    entry
                }));
                if truncated {
                    truncation = Some(Truncation {
                        scope: root,
                        limit: hard_cap,
                    });
                    break;
                }
            }
        } else {
            let (found, truncated) = collect_walk(&roots.scan_root, None, hard_cap);
            entries = found;
            if truncated {
                truncation = Some(Truncation {
                    scope: roots.scan_root.clone(),
                    limit: hard_cap,
                });
            }
        }
        entries.retain(|entry| entry.file_type != super::IndexedFileType::Directory);
        self.append_targets(entries, truncation);
    }

    fn conventional_roots(&self, roots: &RootInfo) -> Vec<(PathBuf, PathBuf)> {
        let mut relative_roots = BTreeSet::new();
        for conventional in CONVENTIONAL_ROOTS {
            let relative = PathBuf::from(conventional);
            if roots.scan_root.join(&relative).is_dir() {
                relative_roots.insert(relative);
            }
        }
        relative_roots.extend(
            self.structural
                .iter()
                .filter(|entry| {
                    entry.file_type == super::IndexedFileType::Directory
                        && entry
                            .relative_path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| CONVENTIONAL_ROOTS.contains(&name))
                })
                .map(|entry| entry.relative_path.clone()),
        );
        let mut selected = Vec::<PathBuf>::new();
        for root in relative_roots {
            if selected.iter().any(|parent| root.starts_with(parent)) {
                continue;
            }
            selected.push(root);
        }
        selected
            .into_iter()
            .map(|relative| {
                let absolute = roots.scan_root.join(&relative);
                (relative, absolute)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::intent::Target;
    use crate::scan::{resolve_roots, FileIndex, ScanOptions};

    #[test]
    fn chaos_one_scans_conventional_roots_inside_workspace_members() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let member = temp.path().join("deep/apps/web");
        let test_file = member.join("tests/one/two/participant.test.js");
        std::fs::create_dir_all(test_file.parent().unwrap_or(&member))?;
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"workspaces":["deep/apps/web"]}"#,
        )?;
        std::fs::write(member.join("package.json"), "{}")?;
        std::fs::write(&test_file, "")?;
        let roots = resolve_roots(&Target::Directory(member));
        let mut index = FileIndex::build(&roots, ScanOptions::default());
        index.build_targets(&roots, 1, 20_000);
        assert!(index.targets.iter().any(|entry| {
            entry.relative_path == Path::new("deep/apps/web/tests/one/two/participant.test.js")
        }));
        Ok(())
    }
}
