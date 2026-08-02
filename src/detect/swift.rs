use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::{Diagnostic, Severity};
use crate::intent::Intent;
use crate::registry::{SWIFT, SWIFT_SOURCE, SWIFT_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

pub struct SwiftDetector;

#[derive(Clone, Debug)]
struct SwiftProject {
    manifest_path: PathBuf,
    relative_directory: PathBuf,
    directory: PathBuf,
    scope: String,
}

impl Detector for SwiftDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let projects = projects(context);
        let targets = executable_targets(context, &projects);
        let mut output = Detection::default();
        for (index, project) in projects.iter().enumerate() {
            match context.invocation.intent {
                Intent::Run => {
                    output
                        .candidates
                        .extend(run_candidates(project, &targets[index]));
                }
                Intent::Build => output.candidates.push(package_action(
                    project,
                    Intent::Build,
                    "build",
                    vec![OsString::from("build")],
                    95,
                )),
                Intent::Test => {
                    let tests_exist = project.directory.join("Tests").is_dir();
                    output.candidates.push(package_action(
                        project,
                        Intent::Test,
                        "test",
                        vec![OsString::from("test")],
                        if tests_exist { 95 } else { 70 },
                    ));
                }
            }
        }
        if projects.is_empty() {
            output.diagnostics.extend(xcode_diagnostics(context));
        }
        output
    }
}

fn projects(context: &ScanCtx<'_>) -> Vec<SwiftProject> {
    let mut projects = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "Package.swift")
        })
        .map(|entry| {
            let manifest_path = entry.relative_path.clone();
            let relative_directory = manifest_path
                .parent()
                .unwrap_or(Path::new(""))
                .to_path_buf();
            let directory = if relative_directory.as_os_str().is_empty() {
                context.roots.scan_root.clone()
            } else {
                context.roots.scan_root.join(&relative_directory)
            };
            let scope = directory.file_name().map_or_else(
                || "swift-package".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            SwiftProject {
                manifest_path,
                relative_directory,
                directory,
                scope,
            }
        })
        .collect::<Vec<_>>();
    projects.sort_by(|left, right| {
        right
            .relative_directory
            .components()
            .count()
            .cmp(&left.relative_directory.components().count())
            .then_with(|| left.manifest_path.cmp(&right.manifest_path))
    });
    projects.dedup_by(|left, right| left.manifest_path == right.manifest_path);
    projects
}

fn executable_targets(
    context: &ScanCtx<'_>,
    projects: &[SwiftProject],
) -> Vec<BTreeMap<String, PathBuf>> {
    let mut targets = vec![BTreeMap::new(); projects.len()];
    for entry in context.index.all_entries().filter(|entry| {
        entry.file_type == IndexedFileType::File
            && entry
                .relative_path
                .file_name()
                .is_some_and(|name| name == "main.swift")
    }) {
        let Some((index, project)) = projects
            .iter()
            .enumerate()
            .find(|(_, project)| entry.relative_path.starts_with(&project.relative_directory))
        else {
            continue;
        };
        let relative = entry
            .relative_path
            .strip_prefix(&project.relative_directory)
            .unwrap_or(&entry.relative_path);
        let components = relative.components().collect::<Vec<_>>();
        let target = match components.as_slice() {
            [sources, main]
                if sources.as_os_str() == "Sources" && main.as_os_str() == "main.swift" =>
            {
                Some(project.scope.clone())
            }
            [sources, target, main]
                if sources.as_os_str() == "Sources" && main.as_os_str() == "main.swift" =>
            {
                Some(target.as_os_str().to_string_lossy().into_owned())
            }
            _ => None,
        };
        if let Some(target) = target {
            targets[index].insert(target, entry.relative_path.clone());
        }
    }
    targets
}

fn run_candidates(project: &SwiftProject, targets: &BTreeMap<String, PathBuf>) -> Vec<Candidate> {
    let multiple = targets.len() > 1;
    targets
        .iter()
        .map(|(target, source)| {
            let mut args = vec![OsString::from("run")];
            if multiple {
                args.push(OsString::from(target));
            }
            let description = "Conventional SwiftPM executable target".to_owned();
            CandidateBuilder::direct_target(
                SWIFT_SOURCE,
                Intent::Run,
                project.directory.clone(),
                target,
            )
            .action_key(format!("swift:{}:run:{target}", project.scope))
            .tool(SWIFT_TOOL)
            .args(args)
            .cwd(project.directory.clone())
            .selection(SelectionPolicy::Automatic)
            .base_points(if multiple { 85 } else { 95 })
            .lifecycle(Lifecycle::Finite)
            .label(format!("Swift executable {target}"))
            .description(&description)
            .evidence_all(project_evidence(project))
            .evidence(Evidence {
                kind: EvidenceKind::Convention,
                reason: format!(
                    "{} is a conventional executable entrypoint",
                    source.display()
                ),
                points: 0,
                source: Some(source.clone()),
            })
            .search(SearchDocument {
                identities: vec!["run".to_owned(), target.clone()],
                target_paths: vec![project.manifest_path.clone(), source.clone()],
                scopes: vec![project.scope.clone()],
                tags: vec!["swift".to_owned(), "spm".to_owned()],
                text: vec![description],
            })
            .build()
            .expect("Swift run candidate registration is valid")
        })
        .collect()
}

fn package_action(
    project: &SwiftProject,
    intent: Intent,
    action: &str,
    args: Vec<OsString>,
    base_points: i32,
) -> Candidate {
    let description = format!("SwiftPM {action} command");
    let mut evidence = project_evidence(project);
    if intent == Intent::Test && project.directory.join("Tests").is_dir() {
        evidence.push(Evidence {
            kind: EvidenceKind::Convention,
            reason: "package contains a Tests directory".to_owned(),
            points: 10,
            source: Some(project.relative_directory.join("Tests")),
        });
    }
    CandidateBuilder::tool_default(SWIFT_SOURCE, intent, project.directory.clone(), action)
        .action_key(format!("swift:{}:{action}", project.scope))
        .tool(SWIFT_TOOL)
        .args(args)
        .cwd(project.directory.clone())
        .selection(SelectionPolicy::Automatic)
        .base_points(base_points)
        .label(format!("Swift package {action}"))
        .description(&description)
        .evidence_all(evidence)
        .search(SearchDocument {
            identities: vec![action.to_owned()],
            target_paths: vec![project.manifest_path.clone()],
            scopes: vec![project.scope.clone()],
            tags: vec!["swift".to_owned(), "spm".to_owned()],
            text: vec![description],
        })
        .build()
        .expect("Swift package candidate registration is valid")
}

fn project_evidence(project: &SwiftProject) -> Vec<Evidence> {
    vec![
        Evidence {
            kind: EvidenceKind::Manifest,
            reason: "project contains Package.swift".to_owned(),
            points: 0,
            source: Some(project.manifest_path.clone()),
        },
        Evidence {
            kind: EvidenceKind::Rule,
            reason: "Package.swift is executable code and was not evaluated".to_owned(),
            points: 0,
            source: Some(project.manifest_path.clone()),
        },
    ]
}

fn xcode_diagnostics(context: &ScanCtx<'_>) -> Vec<Diagnostic> {
    let paths = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::Directory
                && entry
                    .relative_path
                    .extension()
                    .is_some_and(|extension| extension == "xcodeproj")
        })
        .map(|entry| entry.relative_path.clone())
        .collect::<BTreeSet<_>>();
    paths
        .iter()
        .map(|path| Diagnostic {
            detector: SWIFT,
            severity: Severity::Info,
            message:
                "Xcode project has no static scheme and destination data; no command was inferred"
                    .to_owned(),
            source: Some(path.clone()),
        })
        .collect()
}
