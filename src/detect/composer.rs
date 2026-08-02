use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use serde::Deserialize;

use crate::candidate::{
    Candidate, CandidateOrigin, CommandLayer, Evidence, EvidenceKind, Lifecycle, PassthroughStyle,
    SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{COMPOSER, COMPOSER_SOURCE};
use crate::scan::IndexedFileType;

use super::{Detection, Detector, ScanCtx};

pub struct ComposerDetector;

#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct ComposerManifest {
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) scripts: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub(super) require: BTreeMap<String, serde_json::Value>,
    #[serde(default, rename = "require-dev")]
    pub(super) require_dev: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug)]
pub(super) struct ComposerProject {
    pub(super) manifest_path: PathBuf,
    pub(super) directory: PathBuf,
    pub(super) manifest: ComposerManifest,
}

impl Detector for ComposerDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let (projects, diagnostics) = composer_projects(context, true);
        let mut output = Detection {
            candidates: Vec::new(),
            diagnostics,
        };
        for project in projects {
            let mut has_test_script = false;
            for (name, script) in &project.manifest.scripts {
                if !is_executable_script(script) {
                    output.diagnostics.push(Diagnostic::warning(
                        COMPOSER,
                        format!("composer script `{name}` must be a string or string array"),
                        Some(context.roots.scan_root.join(&project.manifest_path)),
                    ));
                    continue;
                }
                let Some((base_points, selection)) = script_policy(context.invocation.intent, name)
                else {
                    continue;
                };
                has_test_script |= matches!(name.as_str(), "test" | "phpunit" | "pest");
                output.candidates.push(script_candidate(
                    context,
                    &project,
                    name,
                    script,
                    base_points,
                    selection,
                ));
            }
            if context.invocation.intent == Intent::Test && !has_test_script {
                output
                    .candidates
                    .extend(vendor_test_candidates(context, &project));
            }
        }
        output
    }
}

pub(super) fn composer_projects(
    context: &ScanCtx<'_>,
    report_diagnostics: bool,
) -> (Vec<ComposerProject>, Vec<Diagnostic>) {
    let mut paths = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "composer.json")
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
                if report_diagnostics {
                    diagnostics.push(Diagnostic::warning(
                        COMPOSER,
                        error.to_string(),
                        Some(absolute),
                    ));
                }
                continue;
            }
        };
        let manifest = match serde_json::from_str::<ComposerManifest>(&contents) {
            Ok(manifest) => manifest,
            Err(error) => {
                if report_diagnostics {
                    diagnostics.push(Diagnostic::warning(
                        COMPOSER,
                        format!("invalid composer.json: {error}"),
                        Some(absolute),
                    ));
                }
                continue;
            }
        };
        projects.push(ComposerProject {
            directory: absolute
                .parent()
                .unwrap_or(&context.roots.scan_root)
                .to_path_buf(),
            manifest_path,
            manifest,
        });
    }
    (projects, diagnostics)
}

fn script_policy(intent: Intent, name: &str) -> Option<(i32, SelectionPolicy)> {
    let canonical = match intent {
        Intent::Run => ["dev", "serve", "start"]
            .into_iter()
            .position(|candidate| candidate == name)
            .map(|index| [95, 80, 70][index]),
        Intent::Build => (name == "build").then_some(95),
        Intent::Test => ["test", "phpunit", "pest"]
            .into_iter()
            .position(|candidate| candidate == name)
            .map(|index| [95, 85, 80][index]),
    };
    canonical
        .map(|points| (points, SelectionPolicy::Automatic))
        .or_else(|| (intent == Intent::Run).then_some((15, SelectionPolicy::ExplicitHint)))
}

fn script_candidate(
    context: &ScanCtx<'_>,
    project: &ComposerProject,
    name: &str,
    script: &serde_json::Value,
    base_points: i32,
    selection: SelectionPolicy,
) -> Candidate {
    let scope = project_scope(project);
    let mut candidate = Candidate::new(
        format!("composer:{scope}:script:{name}"),
        COMPOSER,
        COMPOSER_SOURCE,
        context.invocation.intent,
        name,
        "composer",
        vec![OsString::from("run-script"), OsString::from(name)],
        project.directory.clone(),
        base_points,
        selection,
    );
    candidate.passthrough = PassthroughStyle::DoubleDash;
    candidate.layer = CommandLayer::EcosystemTask;
    candidate.lifecycle =
        if context.invocation.intent == Intent::Run && name == "dev" && is_multi_process(script) {
            Lifecycle::MultiProcess
        } else if context.invocation.intent == Intent::Run
            && matches!(name, "dev" | "serve" | "start")
        {
            Lifecycle::LongRunning
        } else {
            Lifecycle::Finite
        };
    candidate.label = format!("Composer script `{name}`");
    candidate.description = "Declared root Composer script".to_owned();
    candidate.evidence.push(Evidence {
        kind: EvidenceKind::Manifest,
        reason: format!("composer.json declares script `{name}`"),
        points: 0,
        source: Some(project.manifest_path.clone()),
    });
    if candidate.lifecycle == Lifecycle::MultiProcess {
        candidate.evidence.push(Evidence {
            kind: EvidenceKind::Rule,
            reason: "development script conventionally starts multiple processes".to_owned(),
            points: 0,
            source: Some(project.manifest_path.clone()),
        });
    }
    candidate.search = SearchDocument {
        identities: vec![name.to_owned()],
        target_paths: vec![project.manifest_path.clone()],
        scopes: vec![scope],
        tags: vec!["php".to_owned(), "composer".to_owned()],
        text: vec![candidate.description.clone()],
    };
    if has_laravel_evidence(&project.manifest) {
        candidate.search.scopes.push("laravel".to_owned());
        candidate.search.tags.push("laravel".to_owned());
        candidate.evidence.push(Evidence {
            kind: EvidenceKind::Manifest,
            reason: "composer.json declares Laravel framework support".to_owned(),
            points: 0,
            source: Some(project.manifest_path.clone()),
        });
    }
    candidate
}

fn vendor_test_candidates(context: &ScanCtx<'_>, project: &ComposerProject) -> Vec<Candidate> {
    let bound_target = match &context.invocation.target {
        crate::intent::Target::File(target)
            if target
                .extension()
                .is_some_and(|extension| extension == "php")
                && target.starts_with(&project.directory) =>
        {
            target
                .strip_prefix(&project.directory)
                .ok()
                .map(PathBuf::from)
        }
        crate::intent::Target::Directory(_) | crate::intent::Target::File(_) => None,
    };
    ["pest", "phpunit"]
        .into_iter()
        .enumerate()
        .filter_map(|(index, runner)| {
            let relative = PathBuf::from("vendor/bin").join(runner);
            project.directory.join(&relative).is_file().then(|| {
                let scope = project_scope(project);
                let action_suffix = bound_target.as_ref().map_or_else(String::new, |target| {
                    format!(
                        ":target:{}",
                        target.to_string_lossy().replace(['/', '\\'], ":")
                    )
                });
                let action_name = bound_target
                    .as_deref()
                    .and_then(std::path::Path::file_stem)
                    .map_or_else(
                        || runner.to_owned(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                let mut candidate = Candidate::new(
                    format!("composer:{scope}:vendor-test:{runner}{action_suffix}"),
                    COMPOSER,
                    COMPOSER_SOURCE,
                    Intent::Test,
                    action_name,
                    PathBuf::from(".").join(&relative).into_os_string(),
                    bound_target
                        .iter()
                        .map(|target| target.as_os_str().to_owned())
                        .collect(),
                    project.directory.clone(),
                    [85, 80][index],
                    SelectionPolicy::Automatic,
                );
                candidate.origin = CandidateOrigin::Conventional;
                candidate.label = format!("Project-local {runner}");
                candidate.description = format!("Explicit vendor/bin/{runner} test runner");
                candidate.evidence.push(Evidence {
                    kind: EvidenceKind::Convention,
                    reason: format!("found project-local vendor/bin/{runner}"),
                    points: 0,
                    source: Some(relative.clone()),
                });
                candidate.search = SearchDocument {
                    identities: bound_target
                        .as_deref()
                        .and_then(std::path::Path::file_stem)
                        .map(|name| name.to_string_lossy().into_owned())
                        .into_iter()
                        .chain([runner.to_owned(), "test".to_owned()])
                        .collect(),
                    target_paths: bound_target
                        .iter()
                        .cloned()
                        .chain(std::iter::once(relative))
                        .collect(),
                    scopes: vec![scope],
                    tags: vec!["php".to_owned(), "composer".to_owned()],
                    text: vec![candidate.description.clone()],
                };
                if let Some(target) = &bound_target {
                    candidate.evidence.push(Evidence {
                        kind: EvidenceKind::Rule,
                        reason: format!(
                            "bound project-local {runner} provider to {}",
                            target.display()
                        ),
                        points: 20,
                        source: Some(target.clone()),
                    });
                }
                candidate
            })
        })
        .collect()
}

fn is_executable_script(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(_) => true,
        serde_json::Value::Array(values) => {
            !values.is_empty() && values.iter().all(serde_json::Value::is_string)
        }
        _ => false,
    }
}

fn is_multi_process(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(values) => values.len() > 1,
        serde_json::Value::String(value) => {
            value.contains("concurrently")
                || value.contains("Composer\\Config::disableProcessTimeout")
        }
        _ => false,
    }
}

pub(super) fn project_scope(project: &ComposerProject) -> String {
    project
        .manifest
        .name
        .clone()
        .or_else(|| {
            project
                .directory
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| ".".to_owned())
}

pub(super) fn has_laravel_evidence(manifest: &ComposerManifest) -> bool {
    manifest
        .require
        .keys()
        .chain(manifest.require_dev.keys())
        .any(|name| matches!(name.as_str(), "laravel/framework" | "laravel/laravel"))
}
