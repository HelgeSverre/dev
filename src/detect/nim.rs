use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, CandidateOrigin, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::intent::Intent;
use crate::registry::{NIMBLE_TOOL, NIM_SOURCE, NIM_TOOL};
use crate::scan::{IndexEntry, IndexedFileType};

use super::target::{explicitly_anchored, target_scope};
use super::{CandidateBuilder, Detection, Detector, ScanCtx, TargetRunner};

pub struct NimDetector;

impl Detector for NimDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let has_nimble = context.index.all_entries().any(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .extension()
                    .is_some_and(|extension| extension == "nimble")
        });
        if !has_nimble {
            return Detection::default();
        }
        let scope = context.roots.scan_root.file_name().map_or_else(
            || "nim-project".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let mut candidates = Vec::new();
        let main_file = find_main_nim(context);
        match context.invocation.intent {
            Intent::Run => {
                if let Some(ref main) = main_file {
                    candidates.push(nim_run_candidate(context, &scope, main));
                }
            }
            Intent::Build => {
                if let Some(ref main) = main_file {
                    candidates.push(nim_build_candidate(context, &scope, main));
                }
            }
            Intent::Test => {
                candidates.push(nimble_test_candidate(context, &scope));
            }
        }
        Detection {
            candidates,
            diagnostics: Vec::new(),
        }
    }
}

impl TargetRunner for NimDetector {
    fn supports(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
        context.invocation.intent == Intent::Run && is_nim_file(target)
    }

    fn candidate(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        Some(nim_file_candidate(
            &absolute,
            &target.relative_path,
            explicitly_anchored(target, context),
        ))
    }
}

fn find_main_nim(context: &ScanCtx<'_>) -> Option<PathBuf> {
    let mut entries: Vec<_> = context
        .index
        .all_entries()
        .filter(|entry| {
            is_nim_file(entry)
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "main.nim")
        })
        .map(|entry| entry.relative_path.clone())
        .collect();
    entries.sort();
    if !entries.is_empty() {
        return entries.into_iter().next();
    }
    let mut src_entries: Vec<_> = context
        .index
        .all_entries()
        .filter(|entry| is_nim_file(entry) && entry.relative_path.starts_with("src/"))
        .map(|entry| entry.relative_path.clone())
        .collect();
    src_entries.sort();
    src_entries.into_iter().next()
}

fn nim_run_candidate(context: &ScanCtx<'_>, scope: &str, main: &Path) -> Candidate {
    let args = vec![OsString::from("r"), main.as_os_str().to_owned()];
    let description = "Compile and run the Nim project";
    CandidateBuilder::tool_default(
        NIM_SOURCE,
        Intent::Run,
        context.roots.scan_root.to_path_buf(),
        "run",
    )
    .action_key(format!("nim:{}:run", scope))
    .tool(NIM_TOOL)
    .args(args)
    .cwd(context.roots.scan_root.to_path_buf())
    .selection(SelectionPolicy::Automatic)
    .base_points(90)
    .lifecycle(Lifecycle::LongRunning)
    .label("Nim run")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Convention,
        reason: "detected Nim project with main entry point".to_owned(),
        points: 0,
        source: Some(main.to_path_buf()),
    })
    .search(SearchDocument {
        identities: vec!["run".to_owned(), "nim".to_owned()],
        target_paths: vec![main.to_path_buf()],
        scopes: vec![scope.to_owned()],
        tags: vec!["nim".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("Nim run candidate registration is valid")
}

fn nim_build_candidate(context: &ScanCtx<'_>, scope: &str, main: &Path) -> Candidate {
    let args = vec![OsString::from("c"), main.as_os_str().to_owned()];
    let description = "Compile the Nim project";
    CandidateBuilder::tool_default(
        NIM_SOURCE,
        Intent::Build,
        context.roots.scan_root.to_path_buf(),
        "build",
    )
    .action_key(format!("nim:{}:build", scope))
    .tool(NIM_TOOL)
    .args(args)
    .cwd(context.roots.scan_root.to_path_buf())
    .selection(SelectionPolicy::ExplicitHint)
    .base_points(60)
    .label("Nim build")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Convention,
        reason: "detected Nim project with main entry point".to_owned(),
        points: 0,
        source: Some(main.to_path_buf()),
    })
    .search(SearchDocument {
        identities: vec!["build".to_owned(), "compile".to_owned()],
        target_paths: vec![main.to_path_buf()],
        scopes: vec![scope.to_owned()],
        tags: vec!["nim".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("Nim build candidate registration is valid")
}

fn nimble_test_candidate(context: &ScanCtx<'_>, scope: &str) -> Candidate {
    let description = "Run Nimble project tests";
    CandidateBuilder::tool_default(
        NIM_SOURCE,
        Intent::Test,
        context.roots.scan_root.to_path_buf(),
        "test",
    )
    .action_key(format!("nim:{}:test", scope))
    .tool(NIMBLE_TOOL)
    .args([OsString::from("test")])
    .cwd(context.roots.scan_root.to_path_buf())
    .selection(SelectionPolicy::Automatic)
    .base_points(95)
    .label("Nimble test")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: "Nimble project supports conventional test task".to_owned(),
        points: 0,
        source: None,
    })
    .search(SearchDocument {
        identities: vec!["test".to_owned(), "nimble".to_owned()],
        target_paths: vec![PathBuf::from("*.nimble")],
        scopes: vec![scope.to_owned()],
        tags: vec!["nim".to_owned(), "nimble".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("Nimble test candidate registration is valid")
}

fn nim_file_candidate(absolute: &Path, relative: &Path, explicit: bool) -> Candidate {
    let directory = absolute.parent().unwrap_or(Path::new(".")).to_path_buf();
    let filename = absolute
        .file_name()
        .map_or_else(|| OsString::from("main.nim"), std::ffi::OsStr::to_owned);
    let stem = absolute.file_stem().map_or_else(
        || "main".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let description = "Standalone Nim source target";
    CandidateBuilder::direct_target(NIM_SOURCE, Intent::Run, directory.clone(), &stem)
        .action_key(format!(
            "nim:file:{}",
            relative.to_string_lossy().replace(['/', '\\'], ":")
        ))
        .tool(NIM_TOOL)
        .args([OsString::from("r"), filename.clone()])
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
        .label(format!("Nim file {}", relative.display()))
        .description(description)
        .evidence(Evidence {
            kind: EvidenceKind::Rule,
            reason: "selected nim r for a standalone .nim target".to_owned(),
            points: 0,
            source: Some(relative.to_path_buf()),
        })
        .search(SearchDocument {
            identities: vec![stem, filename.to_string_lossy().into_owned()],
            target_paths: vec![PathBuf::from(filename), relative.to_path_buf()],
            scopes: vec![target_scope(relative)],
            tags: vec!["nim".to_owned()],
            text: vec![description.to_owned()],
        })
        .build()
        .expect("Nim file candidate registration is valid")
}

fn is_nim_file(entry: &IndexEntry) -> bool {
    entry.file_type != IndexedFileType::Directory
        && entry
            .relative_path
            .extension()
            .is_some_and(|extension| extension == "nim")
}
