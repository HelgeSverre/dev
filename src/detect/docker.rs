use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::candidate::{
    Candidate, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{DOCKER, DOCKER_SOURCE, DOCKER_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

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
                    DOCKER,
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
                    DOCKER,
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
        for service in manifest.services.keys() {
            if safe_service_name(service) {
                output.candidates.push(compose_up_candidate(
                    &manifest_path,
                    &relative_manifest,
                    Some(service),
                ));
            } else {
                output.diagnostics.push(Diagnostic::warning(
                    DOCKER,
                    format!(
                        "ignoring Compose service `{service}` because the runner would parse it as an option"
                    ),
                    Some(manifest_path.clone()),
                ));
            }
        }
    }
    output
}

fn safe_service_name(name: &str) -> bool {
    !name.starts_with('-')
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
    let label = service.map_or_else(
        || "Compose up".to_owned(),
        |service| format!("Compose service {service}"),
    );
    let description = service.map_or_else(
        || "Starts the services declared by the Compose application".to_owned(),
        |service| format!("Starts declared Compose service `{service}`"),
    );
    CandidateBuilder::tool_default(DOCKER_SOURCE, Intent::Run, directory.clone(), action)
        .action_key(action_key)
        .tool(DOCKER_TOOL)
        .args(args)
        .cwd(directory)
        .selection(SelectionPolicy::ExplicitHint)
        .base_points(if service.is_some() { 45 } else { 40 })
        .lifecycle(Lifecycle::MultiProcess)
        .label(label)
        .description(&description)
        .evidence(Evidence {
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
        })
        .search(SearchDocument {
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
            text: vec![description],
        })
        .build()
        .expect("Docker Compose candidate registration is valid")
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
            let description =
                "Builds the Dockerfile with the current directory as context".to_owned();
            CandidateBuilder::tool_default(DOCKER_SOURCE, Intent::Build, directory.clone(), "build")
                .action_key(format!(
                    "docker:{}:build",
                    normalized_parent(&entry.relative_path)
                ))
                .tool(DOCKER_TOOL)
                .args([OsString::from("build"), OsString::from(".")])
                .cwd(directory)
                .selection(SelectionPolicy::ExplicitHint)
                .base_points(40)
                .label("Dockerfile build")
                .description(&description)
                .evidence(Evidence {
                    kind: EvidenceKind::Manifest,
                    reason: "project contains Dockerfile".to_owned(),
                    points: 0,
                    source: Some(entry.relative_path.clone()),
                })
                .search(SearchDocument {
                    identities: vec!["dockerfile".to_owned(), "build".to_owned()],
                    target_paths: vec![entry.relative_path.clone()],
                    scopes: vec![scope],
                    tags: vec!["docker".to_owned(), "container".to_owned()],
                    text: vec![description],
                })
                .build()
                .expect("Dockerfile candidate registration is valid")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_dash_service_names_are_not_positional_arguments() {
        assert!(!safe_service_name("-d"));
        assert!(!safe_service_name("--build"));
        assert!(safe_service_name("web"));
    }
}
