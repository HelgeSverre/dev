use std::ffi::OsString;
use std::path::PathBuf;

use serde::Deserialize;

use crate::candidate::{
    Candidate, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{RESCRIPT, RESCRIPT_SOURCE, RESCRIPT_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

pub struct RescriptDetector;

#[derive(Clone, Debug, Default, Deserialize)]
struct RescriptManifest {
    name: Option<String>,
}

impl Detector for RescriptDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let (projects, diagnostics) = projects(context);
        let mut output = Detection {
            candidates: Vec::new(),
            diagnostics,
        };
        for project in &projects {
            let mut candidates = project_candidates(context.invocation.intent, project);
            output.candidates.append(&mut candidates);
        }
        output
    }
}

#[derive(Clone, Debug)]
struct RescriptProject {
    manifest_path: PathBuf,
    directory: PathBuf,
    scope: String,
    #[allow(dead_code)]
    manifest: RescriptManifest,
}

fn projects(context: &ScanCtx<'_>) -> (Vec<RescriptProject>, Vec<Diagnostic>) {
    let mut paths = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "rescript.json")
        })
        .map(|entry| entry.relative_path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let mut projects = Vec::new();
    let mut diagnostics = Vec::new();
    for manifest_path in paths {
        let absolute = context.roots.scan_root.join(&manifest_path);
        let contents = match context.index.manifests.read(&absolute) {
            Ok(contents) => contents,
            Err(error) => {
                diagnostics.push(Diagnostic::warning(
                    RESCRIPT,
                    error.to_string(),
                    Some(absolute),
                ));
                continue;
            }
        };
        let manifest = match serde_json::from_str::<RescriptManifest>(&contents) {
            Ok(manifest) => manifest,
            Err(error) => {
                diagnostics.push(Diagnostic::warning(
                    RESCRIPT,
                    format!("invalid rescript.json: {error}"),
                    Some(absolute),
                ));
                continue;
            }
        };
        let directory = absolute
            .parent()
            .unwrap_or(&context.roots.scan_root)
            .to_path_buf();
        let scope = manifest.name.clone().unwrap_or_else(|| {
            directory.file_name().map_or_else(
                || "rescript-project".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        });
        projects.push(RescriptProject {
            manifest_path,
            directory,
            scope,
            manifest,
        });
    }
    (projects, diagnostics)
}

fn project_candidates(intent: Intent, project: &RescriptProject) -> Vec<Candidate> {
    match intent {
        Intent::Run => vec![rescript_dev_candidate(project)],
        Intent::Build => vec![rescript_build_candidate(project)],
        Intent::Test => Vec::new(),
    }
}

fn rescript_dev_candidate(project: &RescriptProject) -> Candidate {
    let description = "Starts the ReScript development server with hot-reloading";
    CandidateBuilder::tool_default(
        RESCRIPT_SOURCE,
        Intent::Run,
        project.directory.clone(),
        "dev",
    )
    .action_key(format!("rescript:{}:dev", project.scope))
    .tool(RESCRIPT_TOOL)
    .args([OsString::from("dev")])
    .cwd(project.directory.clone())
    .selection(SelectionPolicy::Automatic)
    .base_points(95)
    .lifecycle(Lifecycle::LongRunning)
    .label("ReScript dev")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: "rescript.json declares a ReScript project".to_owned(),
        points: 0,
        source: Some(project.manifest_path.clone()),
    })
    .search(SearchDocument {
        identities: vec!["dev".to_owned(), "watch".to_owned()],
        target_paths: vec![project.manifest_path.clone()],
        scopes: vec![project.scope.clone()],
        tags: vec!["rescript".to_owned(), "re".to_owned(), "bs".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("ReScript dev candidate registration is valid")
}

fn rescript_build_candidate(project: &RescriptProject) -> Candidate {
    let description = "Compiles and bundles the ReScript project";
    CandidateBuilder::tool_default(
        RESCRIPT_SOURCE,
        Intent::Build,
        project.directory.clone(),
        "build",
    )
    .action_key(format!("rescript:{}:build", project.scope))
    .tool(RESCRIPT_TOOL)
    .args([OsString::from("build")])
    .cwd(project.directory.clone())
    .selection(SelectionPolicy::Automatic)
    .base_points(95)
    .label("ReScript build")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: "rescript.json declares a ReScript project".to_owned(),
        points: 0,
        source: Some(project.manifest_path.clone()),
    })
    .search(SearchDocument {
        identities: vec!["build".to_owned(), "compile".to_owned()],
        target_paths: vec![project.manifest_path.clone()],
        scopes: vec![project.scope.clone()],
        tags: vec!["rescript".to_owned(), "re".to_owned(), "bs".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("ReScript build candidate registration is valid")
}
