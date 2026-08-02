use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::candidate::Candidate;
use crate::intent::Target;
use crate::query::matcher::TargetIdentityMatcher;
use crate::query::normalize_query;
use crate::scan::{IndexEntry, IndexedFileType};

use super::ScanCtx;

pub trait TargetBinder: Send + Sync {
    fn supports(&self, base: &Candidate, target: &IndexEntry, context: &ScanCtx<'_>) -> bool;
    fn bind(
        &self,
        base: &Candidate,
        target: &IndexEntry,
        context: &ScanCtx<'_>,
    ) -> Option<Candidate>;
}

pub trait TargetRunner: Send + Sync {
    fn supports(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> bool;
    fn candidate(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> Option<Candidate>;
}

struct HintedTarget {
    entry: IndexEntry,
    bindable: bool,
}

pub(super) fn expand(candidates: Vec<Candidate>, context: &ScanCtx<'_>) -> Vec<Candidate> {
    let explicit = explicit_entry(context);
    let hinted_targets = hinted_entries(context);
    let binders = crate::registry::registrations()
        .iter()
        .flat_map(|registration| registration.target_binders.iter().copied())
        .collect::<Vec<_>>();
    let mut claimed = BTreeSet::<PathBuf>::new();
    let mut expanded = Vec::new();

    for base in candidates {
        let mut bound = Vec::new();
        if let Some(target) = explicit.as_ref() {
            for binder in &binders {
                if binder.supports(&base, target, context) {
                    if let Some(candidate) = binder.bind(&base, target, context) {
                        claimed.insert(target.relative_path.clone());
                        bound.push(candidate);
                    }
                }
            }
        } else {
            for target in &hinted_targets {
                if !target.bindable {
                    continue;
                }
                let target = &target.entry;
                for binder in &binders {
                    if binder.supports(&base, target, context) {
                        if let Some(candidate) = binder.bind(&base, target, context) {
                            claimed.insert(target.relative_path.clone());
                            bound.push(candidate);
                        }
                    }
                }
            }
        }
        if explicit.is_none() || bound.is_empty() {
            expanded.push(base);
        }
        expanded.extend(bound);
    }

    let runners = crate::registry::registrations()
        .iter()
        .flat_map(|registration| registration.target_runners.iter().copied())
        .collect::<Vec<_>>();
    if let Some(target) = explicit.as_ref() {
        if !claimed.contains(&target.relative_path) {
            append_runner_candidates(&mut expanded, &runners, target, context);
        }
    } else {
        for target in hinted_targets {
            if !claimed.contains(&target.entry.relative_path) {
                append_runner_candidates(&mut expanded, &runners, &target.entry, context);
            }
        }
    }
    expanded
}

fn append_runner_candidates(
    candidates: &mut Vec<Candidate>,
    runners: &[&dyn TargetRunner],
    target: &IndexEntry,
    context: &ScanCtx<'_>,
) {
    for runner in runners {
        if runner.supports(target, context) {
            if let Some(candidate) = runner.candidate(target, context) {
                candidates.push(candidate);
            }
        }
    }
}

fn hinted_entries(context: &ScanCtx<'_>) -> Vec<HintedTarget> {
    if context.invocation.hints.is_empty() || context.invocation.chaos == 0 {
        return Vec::new();
    }
    let query = normalize_query(&context.invocation.hints);
    let mut matcher = TargetIdentityMatcher::new(&query, context.invocation.chaos);
    context
        .index
        .all_entries()
        .filter(|entry| entry.file_type != IndexedFileType::Directory)
        .filter_map(|entry| {
            let matched = matcher.match_path(&entry.relative_path);
            matched.meaningful.then(|| HintedTarget {
                entry: entry.clone(),
                bindable: matched.bindable,
            })
        })
        .collect()
}

fn explicit_entry(context: &ScanCtx<'_>) -> Option<IndexEntry> {
    let Target::File(path) = &context.invocation.target else {
        return None;
    };
    let relative_path = path
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(path)
        .to_path_buf();
    if let Some(entry) = context.index.find_relative(&relative_path) {
        return Some(entry.clone());
    }
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let file_type = if metadata.file_type().is_symlink() {
        IndexedFileType::Symlink
    } else if metadata.is_file() {
        IndexedFileType::File
    } else {
        return None;
    };
    Some(IndexEntry {
        relative_path,
        file_type,
        executable: executable(path, &metadata),
        size: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

pub(super) fn explicitly_anchored(target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
    let Target::File(path) = &context.invocation.target else {
        return false;
    };
    path.strip_prefix(&context.roots.scan_root).unwrap_or(path) == target.relative_path
}

pub(super) fn target_scope(path: &Path) -> String {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || ".".to_owned(),
            |parent| parent.to_string_lossy().into_owned(),
        )
}

#[cfg(unix)]
fn executable(_path: &Path, metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
fn executable(path: &Path, metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
        && path.extension().is_some_and(|extension| {
            matches!(
                extension.to_string_lossy().to_ascii_lowercase().as_str(),
                "exe" | "com" | "bat" | "cmd"
            )
        })
}

#[cfg(not(any(unix, windows)))]
fn executable(_path: &Path, _metadata: &std::fs::Metadata) -> bool {
    false
}
