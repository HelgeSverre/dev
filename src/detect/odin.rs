use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, CandidateOrigin, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::intent::Intent;
use crate::registry::{ODIN_SOURCE, ODIN_TOOL};
use crate::scan::{IndexEntry, IndexedFileType};

use super::target::{explicitly_anchored, target_scope};
use super::{CandidateBuilder, Detection, Detector, ScanCtx, TargetRunner};

pub struct OdinDetector;

impl Detector for OdinDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut candidates = Vec::new();
        let main_path = find_main_odin(context);
        if main_path.is_some() {
            match context.invocation.intent {
                Intent::Run => candidates.push(odin_run_project(context, main_path.as_deref())),
                Intent::Build => {
                    candidates.push(odin_build_project(context, main_path.as_deref()));
                }
                Intent::Test => {}
            }
        }
        Detection {
            candidates,
            diagnostics: Vec::new(),
        }
    }
}

impl TargetRunner for OdinDetector {
    fn supports(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
        context.invocation.intent == Intent::Run && is_odin_file(target)
    }

    fn candidate(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        Some(odin_file_candidate(
            &absolute,
            &target.relative_path,
            explicitly_anchored(target, context),
        ))
    }
}

fn find_main_odin(context: &ScanCtx<'_>) -> Option<PathBuf> {
    let mut entries: Vec<_> = context
        .index
        .all_entries()
        .filter(|entry| {
            is_odin_file(entry)
                && (entry.relative_path == Path::new("main.odin")
                    || entry.relative_path == Path::new("src/main.odin"))
        })
        .map(|entry| entry.relative_path.clone())
        .collect();
    entries.sort();
    entries.into_iter().next()
}

fn odin_run_project(context: &ScanCtx<'_>, main: Option<&Path>) -> Candidate {
    let scope = context.roots.scan_root.file_name().map_or_else(
        || "odin-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let main_target = main.unwrap_or(Path::new("main.odin"));
    let description = "Compile and run the Odin project from its root";
    CandidateBuilder::tool_default(
        ODIN_SOURCE,
        Intent::Run,
        context.roots.scan_root.to_path_buf(),
        "run",
    )
    .action_key(format!("odin:{}:run", scope))
    .tool(ODIN_TOOL)
    .args([OsString::from("run"), main_target.as_os_str().to_owned()])
    .cwd(context.roots.scan_root.to_path_buf())
    .selection(SelectionPolicy::Automatic)
    .base_points(90)
    .lifecycle(Lifecycle::LongRunning)
    .label("Odin run")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Convention,
        reason: "detected main.odin entry point".to_owned(),
        points: 0,
        source: Some(main_target.to_path_buf()),
    })
    .search(SearchDocument {
        identities: vec!["run".to_owned(), "odin".to_owned()],
        target_paths: vec![main_target.to_path_buf()],
        scopes: vec![scope],
        tags: vec!["odin".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("Odin run candidate registration is valid")
}

fn odin_build_project(context: &ScanCtx<'_>, main: Option<&Path>) -> Candidate {
    let scope = context.roots.scan_root.file_name().map_or_else(
        || "odin-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let main_target = main.unwrap_or(Path::new("main.odin"));
    let description = "Compile the Odin project from its root";
    CandidateBuilder::tool_default(
        ODIN_SOURCE,
        Intent::Build,
        context.roots.scan_root.to_path_buf(),
        "build",
    )
    .action_key(format!("odin:{}:build", scope))
    .tool(ODIN_TOOL)
    .args([OsString::from("build"), main_target.as_os_str().to_owned()])
    .cwd(context.roots.scan_root.to_path_buf())
    .selection(SelectionPolicy::ExplicitHint)
    .base_points(60)
    .label("Odin build")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Convention,
        reason: "detected main.odin entry point".to_owned(),
        points: 0,
        source: Some(main_target.to_path_buf()),
    })
    .search(SearchDocument {
        identities: vec!["build".to_owned(), "odin".to_owned()],
        target_paths: vec![main_target.to_path_buf()],
        scopes: vec![scope],
        tags: vec!["odin".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("Odin build candidate registration is valid")
}

fn odin_file_candidate(absolute: &Path, relative: &Path, explicit: bool) -> Candidate {
    let directory = absolute.parent().unwrap_or(Path::new(".")).to_path_buf();
    let filename = absolute
        .file_name()
        .map_or_else(|| OsString::from("main.odin"), std::ffi::OsStr::to_owned);
    let stem = absolute.file_stem().map_or_else(
        || "main".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let description = "Standalone Odin source target";
    CandidateBuilder::direct_target(ODIN_SOURCE, Intent::Run, directory.clone(), &stem)
        .action_key(format!(
            "odin:file:{}",
            relative.to_string_lossy().replace(['/', '\\'], ":")
        ))
        .tool(ODIN_TOOL)
        .args([OsString::from("run"), filename.clone()])
        .cwd(directory)
        .selection(if explicit {
            SelectionPolicy::Automatic
        } else {
            SelectionPolicy::ExplicitHint
        })
        .base_points(if explicit { 90 } else { 25 })
        .lifecycle(Lifecycle::LongRunning)
        .origin(if explicit {
            CandidateOrigin::Declared
        } else {
            CandidateOrigin::Synthetic
        })
        .label(format!("Odin file {}", relative.display()))
        .description(description)
        .evidence(Evidence {
            kind: EvidenceKind::Rule,
            reason: "selected odin run for a standalone .odin target".to_owned(),
            points: 0,
            source: Some(relative.to_path_buf()),
        })
        .search(SearchDocument {
            identities: vec![stem, filename.to_string_lossy().into_owned()],
            target_paths: vec![PathBuf::from(filename), relative.to_path_buf()],
            scopes: vec![target_scope(relative)],
            tags: vec!["odin".to_owned()],
            text: vec![description.to_owned()],
        })
        .build()
        .expect("Odin file candidate registration is valid")
}

fn is_odin_file(entry: &IndexEntry) -> bool {
    entry.file_type != IndexedFileType::Directory
        && entry
            .relative_path
            .extension()
            .is_some_and(|extension| extension == "odin")
}
