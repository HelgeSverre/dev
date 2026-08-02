use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::candidate::{
    Candidate, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::scan::IndexedFileType;

use super::{Detection, Detector, ScanCtx};

const COMPOSE_FILES: &[&str] = &[
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

pub struct DockerDetector;

#[derive(Clone, Debug, Default, Deserialize)]
struct ComposeFile {
    #[serde(default)]
    services: BTreeMap<String, serde_yaml::Value>,
}

impl Detector for DockerDetector {
    fn name(&self) -> &'static str {
        "docker"
    }

    fn synonyms(&self) -> &'static [&'static str] {
        &["docker", "compose", "container"]
    }

    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        match context.invocation.intent {
            Intent::Run => compose_candidates(context),
            Intent::Build => Detection {
                candidates: dockerfile_candidates(context),
                diagnostics: Vec::new(),
            },
            Intent::Test => Detection::default(),
        }
    }
}

fn compose_candidates(context: &ScanCtx<'_>) -> Detection {
    let mut output = Detection::default();
    for (manifest_path, relative_manifest) in compose_files(context) {
        let contents = match context.index.manifests.read(&manifest_path) {
            Ok(contents) => contents,
            Err(error) => {
                output.diagnostics.push(Diagnostic::warning(
                    "docker",
                    error.to_string(),
                    Some(manifest_path),
                ));
                continue;
            }
        };
        let manifest = match serde_yaml::from_str::<ComposeFile>(&contents) {
            Ok(manifest) => manifest,
            Err(error) => {
                output.diagnostics.push(Diagnostic::warning(
                    "docker",
                    format!("invalid Compose YAML: {error}"),
                    Some(manifest_path),
                ));
                continue;
            }
        };
        output.candidates.push(compose_up_candidate(
            &manifest_path,
            &relative_manifest,
            None,
        ));
        output
            .candidates
            .extend(manifest.services.keys().map(|service| {
                compose_up_candidate(&manifest_path, &relative_manifest, Some(service))
            }));
    }
    output
}

fn compose_files(context: &ScanCtx<'_>) -> Vec<(PathBuf, PathBuf)> {
    let mut by_directory = BTreeMap::<PathBuf, (usize, PathBuf)>::new();
    for entry in context.index.all_entries().filter(|entry| {
        entry.file_type == IndexedFileType::File
            && entry
                .relative_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| COMPOSE_FILES.contains(&name))
    }) {
        let Some(filename) = entry
            .relative_path
            .file_name()
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        let priority = COMPOSE_FILES
            .iter()
            .position(|candidate| *candidate == filename)
            .unwrap_or(usize::MAX);
        let directory = entry
            .relative_path
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let absolute = context.roots.scan_root.join(&entry.relative_path);
        by_directory
            .entry(directory)
            .and_modify(|current| {
                if priority < current.0 {
                    *current = (priority, absolute.clone());
                }
            })
            .or_insert((priority, absolute));
    }
    by_directory
        .into_values()
        .map(|(_, path)| {
            let relative = path
                .strip_prefix(&context.roots.scan_root)
                .unwrap_or(&path)
                .to_path_buf();
            (path, relative)
        })
        .collect()
}

fn compose_up_candidate(
    manifest_path: &Path,
    relative_manifest: &Path,
    service: Option<&String>,
) -> Candidate {
    let directory = manifest_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let scope = directory.file_name().map_or_else(
        || "compose-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mut args = vec![OsString::from("compose"), OsString::from("up")];
    args.extend(service.map(OsString::from));
    let action_key = service.map_or_else(
        || format!("docker:{}:compose-up", normalized_parent(relative_manifest)),
        |service| {
            format!(
                "docker:{}:compose-up:target:{service}",
                normalized_parent(relative_manifest)
            )
        },
    );
    let action = service.map_or("compose", String::as_str);
    let mut candidate = Candidate::new(
        action_key,
        "docker",
        Intent::Run,
        action,
        "docker",
        args,
        directory,
        if service.is_some() { 45 } else { 40 },
        SelectionPolicy::ExplicitHint,
    );
    candidate.lifecycle = Lifecycle::MultiProcess;
    candidate.label = service.map_or_else(
        || "Compose up".to_owned(),
        |service| format!("Compose service {service}"),
    );
    candidate.description = service.map_or_else(
        || "Starts the services declared by the Compose application".to_owned(),
        |service| format!("Starts declared Compose service `{service}`"),
    );
    candidate.evidence.push(Evidence {
        kind: EvidenceKind::Manifest,
        reason: service.map_or_else(
            || {
                format!(
                    "{} is a standard Compose manifest",
                    relative_manifest.display()
                )
            },
            |service| {
                format!(
                    "{} declares service `{service}`",
                    relative_manifest.display()
                )
            },
        ),
        points: 0,
        source: Some(relative_manifest.to_path_buf()),
    });
    candidate.search = SearchDocument {
        identities: service.map_or_else(
            || vec!["compose".to_owned(), "up".to_owned()],
            |service| vec![service.clone()],
        ),
        target_paths: service.map_or_else(
            || vec![relative_manifest.to_path_buf()],
            |service| vec![PathBuf::from(service)],
        ),
        scopes: vec![scope],
        tags: vec!["docker".to_owned(), "compose".to_owned()],
        text: vec![candidate.description.clone()],
    };
    candidate
}

fn dockerfile_candidates(context: &ScanCtx<'_>) -> Vec<Candidate> {
    context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "Dockerfile")
        })
        .map(|entry| {
            let absolute = context.roots.scan_root.join(&entry.relative_path);
            let directory = absolute.parent().unwrap_or(Path::new(".")).to_path_buf();
            let scope = directory.file_name().map_or_else(
                || "docker-project".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            let mut candidate = Candidate::new(
                format!("docker:{}:build", normalized_parent(&entry.relative_path)),
                "docker",
                Intent::Build,
                "build",
                "docker",
                vec![OsString::from("build"), OsString::from(".")],
                directory,
                40,
                SelectionPolicy::ExplicitHint,
            );
            candidate.label = "Dockerfile build".to_owned();
            candidate.description =
                "Builds the Dockerfile with the current directory as context".to_owned();
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Manifest,
                reason: "project contains Dockerfile".to_owned(),
                points: 0,
                source: Some(entry.relative_path.clone()),
            });
            candidate.search = SearchDocument {
                identities: vec!["dockerfile".to_owned(), "build".to_owned()],
                target_paths: vec![entry.relative_path.clone()],
                scopes: vec![scope],
                tags: vec!["docker".to_owned(), "container".to_owned()],
                text: vec![candidate.description.clone()],
            };
            candidate
        })
        .collect()
}

fn normalized_parent(path: &Path) -> String {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || "root".to_owned(),
            |parent| parent.to_string_lossy().replace(['/', '\\'], ":"),
        )
}
