use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::candidate::{
    Availability, Candidate, CandidateOrigin, CommandLayer, Evidence, EvidenceKind, SearchDocument,
    SelectionPolicy,
};
use crate::intent::Intent;
use crate::path::resolve_program;
use crate::registry::{PYTHON_FILE, PYTHON_FILE_SOURCE};
use crate::scan::{IndexEntry, IndexedFileType};

use super::target::{explicitly_anchored, target_scope};
use super::{Detection, Detector, ScanCtx, TargetRunner};

pub struct PythonFileDetector;

impl Detector for PythonFileDetector {
    fn detect(&self, _context: &ScanCtx<'_>) -> Detection {
        Detection::default()
    }
}

impl TargetRunner for PythonFileDetector {
    fn supports(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
        context.invocation.intent == Intent::Run && is_python_file(target)
    }

    fn candidate(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        Some(file_candidate(
            &absolute,
            &target.relative_path,
            explicitly_anchored(target, context),
        ))
    }
}

fn file_candidate(absolute: &Path, relative_to_scan: &Path, explicit: bool) -> Candidate {
    let directory = absolute.parent().unwrap_or(Path::new(".")).to_path_buf();
    let filename = absolute
        .file_name()
        .map_or_else(|| OsString::from("script.py"), std::ffi::OsStr::to_owned);
    let stem = absolute.file_stem().map_or_else(
        || "python-script".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let (program, runner_reason) = python_interpreter(&directory);
    let mut candidate = Candidate::new(
        format!(
            "python-file:{}",
            relative_to_scan.to_string_lossy().replace(['/', '\\'], ":")
        ),
        PYTHON_FILE,
        PYTHON_FILE_SOURCE,
        Intent::Run,
        &stem,
        program,
        vec![filename.clone()],
        directory,
        if explicit { 90 } else { 25 },
        if explicit {
            SelectionPolicy::Automatic
        } else {
            SelectionPolicy::ExplicitHint
        },
    );
    candidate.origin = if explicit {
        CandidateOrigin::Declared
    } else {
        CandidateOrigin::Synthetic
    };
    candidate.layer = CommandLayer::DirectTarget;
    candidate.label = format!("Python file {}", relative_to_scan.display());
    candidate.description = format!("Standalone Python target using {runner_reason}");
    candidate.evidence.push(Evidence {
        kind: EvidenceKind::Rule,
        reason: format!("selected {runner_reason} for Python target"),
        points: 0,
        source: Some(relative_to_scan.to_path_buf()),
    });
    candidate.search = SearchDocument {
        identities: vec![stem, filename.to_string_lossy().into_owned()],
        target_paths: vec![PathBuf::from(&filename), relative_to_scan.to_path_buf()],
        scopes: vec![target_scope(relative_to_scan)],
        tags: vec!["python".to_owned(), "py".to_owned()],
        text: vec![candidate.description.clone()],
    };
    candidate
}

fn python_interpreter(cwd: &Path) -> (OsString, String) {
    if let Some(virtual_environment) = std::env::var_os("VIRTUAL_ENV") {
        let root = PathBuf::from(virtual_environment);
        #[cfg(windows)]
        let interpreter = root.join("Scripts/python.exe");
        #[cfg(not(windows))]
        let interpreter = root.join("bin/python");
        if matches!(
            resolve_program(interpreter.as_os_str(), cwd, &BTreeMap::new()),
            Availability::Available { .. }
        ) {
            return (
                interpreter.into_os_string(),
                "the active virtual environment interpreter".to_owned(),
            );
        }
    }
    if matches!(
        resolve_program(OsStr::new("python3"), cwd, &BTreeMap::new()),
        Availability::Available { .. }
    ) {
        return (OsString::from("python3"), "python3".to_owned());
    }
    (OsString::from("python"), "python".to_owned())
}

fn is_python_file(entry: &IndexEntry) -> bool {
    entry.file_type != IndexedFileType::Directory && has_python_extension(&entry.relative_path)
}

fn has_python_extension(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "py")
}
