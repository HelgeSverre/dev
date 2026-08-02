use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, CandidateOrigin, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::intent::Intent;
use crate::registry::{WREN_SOURCE, WREN_TOOL};
use crate::scan::{IndexEntry, IndexedFileType};

use super::target::{explicitly_anchored, target_scope};
use super::{CandidateBuilder, Detection, Detector, ScanCtx, TargetRunner};

pub struct WrenDetector;

impl Detector for WrenDetector {
    fn detect(&self, _context: &ScanCtx<'_>) -> Detection {
        Detection::default()
    }
}

impl TargetRunner for WrenDetector {
    fn supports(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
        context.invocation.intent == Intent::Run && is_wren_file(target)
    }

    fn candidate(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        Some(wren_file_candidate(
            &absolute,
            &target.relative_path,
            explicitly_anchored(target, context),
        ))
    }
}

fn wren_file_candidate(absolute: &Path, relative: &Path, explicit: bool) -> Candidate {
    let directory = absolute.parent().unwrap_or(Path::new(".")).to_path_buf();
    let filename = absolute
        .file_name()
        .map_or_else(|| OsString::from("main.wren"), std::ffi::OsStr::to_owned);
    let stem = absolute.file_stem().map_or_else(
        || "main".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let description = "Standalone Wren source target";
    CandidateBuilder::direct_target(WREN_SOURCE, Intent::Run, directory.clone(), &stem)
        .action_key(format!(
            "wren:file:{}",
            relative.to_string_lossy().replace(['/', '\\'], ":")
        ))
        .tool(WREN_TOOL)
        .args([filename.clone()])
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
        .passthrough(crate::candidate::PassthroughStyle::Append)
        .label(format!("Wren file {}", relative.display()))
        .description(description)
        .evidence(Evidence {
            kind: EvidenceKind::Rule,
            reason: "selected wren_cli for a standalone .wren target".to_owned(),
            points: 0,
            source: Some(relative.to_path_buf()),
        })
        .search(SearchDocument {
            identities: vec![stem, filename.to_string_lossy().into_owned()],
            target_paths: vec![PathBuf::from(filename), relative.to_path_buf()],
            scopes: vec![target_scope(relative)],
            tags: vec!["wren".to_owned()],
            text: vec![description.to_owned()],
        })
        .build()
        .expect("Wren file candidate registration is valid")
}

fn is_wren_file(entry: &IndexEntry) -> bool {
    entry.file_type != IndexedFileType::Directory
        && entry
            .relative_path
            .extension()
            .is_some_and(|extension| extension == "wren")
}
