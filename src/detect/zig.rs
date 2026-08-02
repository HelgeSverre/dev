use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, CandidateOrigin, Evidence, EvidenceKind, Lifecycle, PassthroughStyle,
    SearchDocument, SelectionPolicy,
};
use crate::intent::Intent;
use crate::scan::{IndexEntry, IndexedFileType};

use super::target::explicitly_anchored;
use super::{Detection, Detector, ScanCtx, TargetRunner};

pub struct ZigDetector;

impl Detector for ZigDetector {
    fn name(&self) -> &'static str {
        "zig"
    }

    fn synonyms(&self) -> &'static [&'static str] {
        &["zig"]
    }

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
    let mut candidate = Candidate::new(
        format!("zig:{scope}:build:{action}"),
        "zig",
        intent,
        action,
        "zig",
        args,
        directory,
        if intent == Intent::Build { 95 } else { 85 },
        SelectionPolicy::Automatic,
    );
    candidate.passthrough = passthrough;
    candidate.lifecycle = if intent == Intent::Run {
        Lifecycle::LongRunning
    } else {
        Lifecycle::Finite
    };
    candidate.label = format!("Zig build {action}");
    candidate.description = format!("Zig build-system {action} step");
    candidate.evidence.extend([
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
    ]);
    candidate.search = SearchDocument {
        identities: vec![action.to_owned()],
        target_paths: vec![PathBuf::from("build.zig")],
        scopes: vec![scope],
        tags: vec!["zig".to_owned()],
        text: vec![candidate.description.clone()],
    };
    candidate
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
    let mut candidate = Candidate::new(
        format!(
            "zig:file:{}",
            relative.to_string_lossy().replace(['/', '\\'], ":")
        ),
        "zig",
        Intent::Run,
        &stem,
        "zig",
        vec![OsString::from("run"), filename.clone()],
        directory,
        if explicit { 95 } else { 25 },
        if explicit {
            SelectionPolicy::Automatic
        } else {
            SelectionPolicy::ExplicitHint
        },
    );
    candidate.passthrough = PassthroughStyle::DoubleDash;
    candidate.origin = CandidateOrigin::Synthetic;
    candidate.label = format!("Zig file {}", relative.display());
    candidate.description = "Standalone Zig source target".to_owned();
    candidate.evidence.push(Evidence {
        kind: EvidenceKind::Rule,
        reason: "selected zig run for a standalone .zig target".to_owned(),
        points: 0,
        source: Some(relative.to_path_buf()),
    });
    candidate.search = SearchDocument {
        identities: vec![stem, filename.to_string_lossy().into_owned()],
        target_paths: vec![PathBuf::from(filename), relative.to_path_buf()],
        scopes: relative
            .parent()
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        tags: vec!["zig".to_owned()],
        text: vec![candidate.description.clone()],
    };
    candidate
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
