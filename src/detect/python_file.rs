use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::candidate::{
    Availability, Candidate, CandidateOrigin, Evidence, EvidenceKind, SearchDocument,
    SelectionPolicy,
};
use crate::intent::{Intent, Target};
use crate::path::resolve_program;
use crate::query::{match_candidate, normalize_query, MatchClass};
use crate::scan::{IndexEntry, IndexedFileType};

use super::{Detection, Detector, ScanCtx};

pub struct PythonFileDetector;

impl Detector for PythonFileDetector {
    fn name(&self) -> &'static str {
        "python-file"
    }

    fn synonyms(&self) -> &'static [&'static str] {
        &["python", "py"]
    }

    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        if context.invocation.intent != Intent::Run {
            return Detection::default();
        }
        if let Target::File(path) = &context.invocation.target {
            if has_python_extension(path) {
                let relative = path.strip_prefix(&context.roots.scan_root).unwrap_or(path);
                return Detection {
                    candidates: vec![file_candidate(path, relative, true)],
                    diagnostics: Vec::new(),
                };
            }
        }
        if context.invocation.hints.is_empty() {
            return Detection::default();
        }

        let query = normalize_query(&context.invocation.hints);
        let mut candidates = context
            .index
            .all_entries()
            .filter(|entry| is_python_file(entry))
            .map(|entry| {
                let absolute = context.roots.scan_root.join(&entry.relative_path);
                file_candidate(&absolute, &entry.relative_path, false)
            })
            .filter(|candidate| {
                let matched = match_candidate(candidate, &query, context.invocation.chaos);
                matched.highest_class == Some(MatchClass::Identity)
                    && matched.matched_meaningful_terms > 0
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.action_key.cmp(&right.action_key));
        candidates.dedup_by(|left, right| left.action_key == right.action_key);
        Detection {
            candidates,
            diagnostics: Vec::new(),
        }
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
        "python-file",
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
    candidate.origin = CandidateOrigin::Synthetic;
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
        scopes: relative_to_scan
            .parent()
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
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
    entry.file_type == IndexedFileType::File && has_python_extension(&entry.relative_path)
}

fn has_python_extension(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "py")
}
