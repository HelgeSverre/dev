use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, CandidateOrigin, Evidence, EvidenceKind, Lifecycle, PassthroughStyle,
    SearchDocument, SelectionPolicy,
};
use crate::intent::Intent;
use crate::registry::{ZIG_SOURCE, ZIG_TOOL};
use crate::scan::{IndexEntry, IndexedFileType};

use super::target::{explicitly_anchored, target_scope};
use super::{CandidateBuilder, Detection, Detector, ScanCtx, TargetRunner};

pub struct ZigDetector;

impl Detector for ZigDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let candidates = build_projects(context)
            .into_iter()
            .map(|path| build_candidate(context.invocation.intent, &path))
            .collect::<Vec<_>>();
        Detection {
            candidates,
            diagnostics: Vec::new(),
        }
    }
}

impl TargetRunner for ZigDetector {
    fn supports(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
        context.invocation.intent == Intent::Run && is_zig_file(target)
    }

    fn candidate(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        Some(standalone_candidate(
            &absolute,
            &target.relative_path,
            explicitly_anchored(target, context),
        ))
    }
}

fn build_projects(context: &ScanCtx<'_>) -> Vec<PathBuf> {
    let mut paths = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "build.zig")
        })
        .map(|entry| context.roots.scan_root.join(&entry.relative_path))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn build_candidate(intent: Intent, build_file: &Path) -> Candidate {
    let directory = build_file.parent().unwrap_or(Path::new(".")).to_path_buf();
    let scope = directory.file_name().map_or_else(
        || "zig-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let (action, args, passthrough) = match intent {
        Intent::Run => (
            "run",
            vec![OsString::from("build"), OsString::from("run")],
            PassthroughStyle::DoubleDash,
        ),
        Intent::Build => (
            "build",
            vec![OsString::from("build")],
            PassthroughStyle::Append,
        ),
        Intent::Test => (
            "test",
            vec![OsString::from("build"), OsString::from("test")],
            PassthroughStyle::Append,
        ),
    };
    let description = format!("Zig build-system {action} step");
    CandidateBuilder::tool_default(ZIG_SOURCE, intent, directory.clone(), action)
        .action_key(format!("zig:{scope}:build:{action}"))
        .tool(ZIG_TOOL)
        .args(args)
        .cwd(directory)
        .selection(SelectionPolicy::Automatic)
        .base_points(if intent == Intent::Build { 95 } else { 85 })
        .passthrough(passthrough)
        .lifecycle(if intent == Intent::Run {
            Lifecycle::LongRunning
        } else {
            Lifecycle::Finite
        })
        .label(format!("Zig build {action}"))
        .description(&description)
        .evidence_all([
            Evidence {
                kind: EvidenceKind::Manifest,
                reason: "project contains build.zig".to_owned(),
                points: 0,
                source: Some(build_file.to_path_buf()),
            },
            Evidence {
                kind: EvidenceKind::Rule,
                reason: "build.zig is executable code; custom steps were not evaluated".to_owned(),
                points: 0,
                source: Some(build_file.to_path_buf()),
            },
        ])
        .search(SearchDocument {
            identities: vec![action.to_owned()],
            target_paths: vec![PathBuf::from("build.zig")],
            scopes: vec![scope],
            tags: vec!["zig".to_owned()],
            text: vec![description],
        })
        .build()
        .expect("Zig build candidate registration is valid")
}

fn standalone_candidate(absolute: &Path, relative: &Path, explicit: bool) -> Candidate {
    let directory = absolute.parent().unwrap_or(Path::new(".")).to_path_buf();
    let filename = absolute
        .file_name()
        .map_or_else(|| OsString::from("main.zig"), std::ffi::OsStr::to_owned);
    let stem = absolute.file_stem().map_or_else(
        || "main".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let description = "Standalone Zig source target".to_owned();
    CandidateBuilder::direct_target(ZIG_SOURCE, Intent::Run, directory.clone(), &stem)
        .action_key(format!(
            "zig:file:{}",
            relative.to_string_lossy().replace(['/', '\\'], ":")
        ))
        .tool(ZIG_TOOL)
        .args([OsString::from("run"), filename.clone()])
        .cwd(directory)
        .selection(if explicit {
            SelectionPolicy::Automatic
        } else {
            SelectionPolicy::ExplicitHint
        })
        .base_points(if explicit { 95 } else { 25 })
        .passthrough(PassthroughStyle::DoubleDash)
        .origin(if explicit {
            CandidateOrigin::Declared
        } else {
            CandidateOrigin::Synthetic
        })
        .label(format!("Zig file {}", relative.display()))
        .description(&description)
        .evidence(Evidence {
            kind: EvidenceKind::Rule,
            reason: "selected zig run for a standalone .zig target".to_owned(),
            points: 0,
            source: Some(relative.to_path_buf()),
        })
        .search(SearchDocument {
            identities: vec![stem, filename.to_string_lossy().into_owned()],
            target_paths: vec![PathBuf::from(filename), relative.to_path_buf()],
            scopes: vec![target_scope(relative)],
            tags: vec!["zig".to_owned()],
            text: vec![description],
        })
        .build()
        .expect("Zig file candidate registration is valid")
}

fn is_zig_file(entry: &IndexEntry) -> bool {
    entry.file_type != IndexedFileType::Directory
        && entry
            .relative_path
            .extension()
            .is_some_and(|extension| extension == "zig")
        && entry
            .relative_path
            .file_name()
            .is_none_or(|name| name != "build.zig")
}
