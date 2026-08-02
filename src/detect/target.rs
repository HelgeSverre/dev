use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::candidate::{Candidate, SearchDocument, SelectionPolicy};
use crate::intent::{Intent, Target};
use crate::query::{match_candidate, normalize, normalize_query, MatchClass};
use crate::scan::{IndexEntry, IndexedFileType};

use super::node::NodeTestBinder;
use super::{
    DartDetector, PhpFileDetector, PythonFileDetector, ScanCtx, ShellDetector, ZigDetector,
};

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

pub(super) fn expand(candidates: Vec<Candidate>, context: &ScanCtx<'_>) -> Vec<Candidate> {
    let explicit = explicit_entry(context);
    let hinted_targets = hinted_entries(context);
    let binders: [&dyn TargetBinder; 1] = [&NodeTestBinder];
    let mut claimed = BTreeSet::<PathBuf>::new();
    let mut expanded = Vec::new();

    for base in candidates {
        let mut bound = Vec::new();
        if let Some(target) = explicit.as_ref() {
            for binder in binders {
                if binder.supports(&base, target, context) {
                    if let Some(candidate) = binder.bind(&base, target, context) {
                        claimed.insert(target.relative_path.clone());
                        bound.push(candidate);
                    }
                }
            }
        } else {
            for target in &hinted_targets {
                if !binder_hint_matches(target, context) {
                    continue;
                }
                for binder in binders {
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

    let runners: [&dyn TargetRunner; 5] = [
        &PhpFileDetector,
        &PythonFileDetector,
        &ShellDetector,
        &ZigDetector,
        &DartDetector,
    ];
    if let Some(target) = explicit.as_ref() {
        if !claimed.contains(&target.relative_path) {
            append_runner_candidates(&mut expanded, &runners, target, context);
        }
    } else {
        for target in hinted_targets {
            if !claimed.contains(&target.relative_path) {
                append_runner_candidates(&mut expanded, &runners, &target, context);
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

fn hinted_entries(context: &ScanCtx<'_>) -> Vec<IndexEntry> {
    if context.invocation.hints.is_empty() || context.invocation.chaos == 0 {
        return Vec::new();
    }
    context
        .index
        .all_entries()
        .filter(|entry| entry.file_type != IndexedFileType::Directory)
        .filter(|entry| target_hint_matches(entry, context))
        .cloned()
        .collect()
}

fn target_hint_matches(target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
    let query = normalize_query(&context.invocation.hints);
    let candidate = target_search_candidate(target, &context.roots.scan_root);
    let matched = match_candidate(&candidate, &query, context.invocation.chaos);
    matched.highest_class == Some(MatchClass::Identity) && matched.matched_meaningful_terms > 0
}

fn binder_hint_matches(target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
    let query = normalize_query(&context.invocation.hints);
    let candidate = target_search_candidate(target, &context.roots.scan_root);
    let matched = match_candidate(&candidate, &query, context.invocation.chaos);
    matched.terms.iter().any(|term| {
        term.class == MatchClass::Identity
            && !matches!(
                normalize(&term.hint).compact.as_str(),
                "run" | "build" | "test" | "tests" | "spec"
            )
    })
}

fn target_search_candidate(target: &IndexEntry, scan_root: &Path) -> Candidate {
    let filename = target.relative_path.file_name().map_or_else(
        || "target".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let stem = target.relative_path.file_stem().map_or_else(
        || filename.clone(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mut candidate = Candidate::new(
        "target-index",
        "target-index",
        Intent::Run,
        &stem,
        "target-index",
        Vec::new(),
        scan_root.to_path_buf(),
        0,
        SelectionPolicy::ExplicitHint,
    );
    candidate.search = SearchDocument {
        identities: vec![stem, filename],
        target_paths: vec![target.relative_path.clone()],
        ..SearchDocument::default()
    };
    candidate
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
        executable: executable(&metadata),
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

#[cfg(unix)]
fn executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &std::fs::Metadata) -> bool {
    false
}
