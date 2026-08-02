use std::path::{Path, PathBuf};

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
            for conventional in CONVENTIONAL_ROOTS {
                let root = roots.scan_root.join(conventional);
                if !root.is_dir() {
                    continue;
                }
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
                    entry.relative_path = PathBuf::from(conventional).join(entry.relative_path);
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
}

#[allow(dead_code)]
fn _is_conventional(path: &Path) -> bool {
    path.components().next().is_some_and(|component| {
        CONVENTIONAL_ROOTS
            .iter()
            .any(|root| component.as_os_str() == std::ffi::OsStr::new(root))
    })
}
