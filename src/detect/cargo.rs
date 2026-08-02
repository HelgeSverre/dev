use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::candidate::{
    Candidate, CandidateOrigin, CommandLayer, Evidence, EvidenceKind, SearchDocument,
    SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{WorkspaceContribution, WorkspaceContributor, CARGO, CARGO_SOURCE};
use crate::scan::IndexedFileType;

use super::{Detection, Detector, ScanCtx};

pub struct CargoDetector;
pub struct CargoWorkspaceContributor;

impl WorkspaceContributor for CargoWorkspaceContributor {
    fn is_workspace(&self, root: &Path) -> bool {
        std::fs::read_to_string(root.join("Cargo.toml"))
            .ok()
            .and_then(|contents| toml::from_str::<toml::Value>(&contents).ok())
            .is_some_and(|manifest| manifest.get("workspace").is_some())
    }

    fn scan_contribution(&self, root: &Path) -> WorkspaceContribution {
        let Some(workspace) = std::fs::read_to_string(root.join("Cargo.toml"))
            .ok()
            .and_then(|contents| toml::from_str::<toml::Value>(&contents).ok())
            .and_then(|manifest| manifest.get("workspace").cloned())
        else {
            return WorkspaceContribution::default();
        };
        let mut contribution = WorkspaceContribution {
            includes: workspace
                .get("members")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .chain(
                    workspace
                        .get("default-members")
                        .and_then(toml::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(toml::Value::as_str),
                )
                .map(|pattern| append_manifest(pattern, "Cargo.toml"))
                .collect(),
            excludes: workspace
                .get("exclude")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .map(|pattern| append_manifest(pattern, "Cargo.toml"))
                .collect(),
        };
        contribution.includes.sort();
        contribution.includes.dedup();
        contribution.excludes.sort();
        contribution.excludes.dedup();
        contribution
    }
}

fn append_manifest(pattern: &str, manifest: &str) -> String {
    format!("{}/{manifest}", pattern.trim_end_matches(['/', '\\']))
}

#[derive(Clone, Debug, Deserialize)]
struct CargoManifest {
    package: Option<CargoPackage>,
    workspace: Option<toml::Value>,
    #[serde(default)]
    bin: Vec<CargoTarget>,
    #[serde(default)]
    example: Vec<CargoTarget>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CargoPackage {
    name: String,
    default_run: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct CargoTarget {
    name: Option<String>,
    path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct ExecutableTarget {
    name: String,
    path: PathBuf,
    example: bool,
}

impl Detector for CargoDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut output = Detection::default();
        let (manifests, diagnostics) = cargo_manifests(context);
        output.diagnostics.extend(diagnostics);
        let workspace_root = manifests.iter().find_map(|(path, manifest)| {
            manifest.workspace.as_ref().map(|_| {
                context
                    .roots
                    .scan_root
                    .join(path)
                    .parent()
                    .unwrap_or(&context.roots.scan_root)
                    .to_path_buf()
            })
        });
        let virtual_workspace_root = manifests.iter().find_map(|(path, manifest)| {
            (manifest.workspace.is_some() && manifest.package.is_none()).then(|| {
                context
                    .roots
                    .scan_root
                    .join(path)
                    .parent()
                    .unwrap_or(&context.roots.scan_root)
                    .to_path_buf()
            })
        });

        for (manifest_path, manifest) in manifests {
            let absolute_manifest = context.roots.scan_root.join(&manifest_path);
            let package_directory = absolute_manifest
                .parent()
                .unwrap_or(&context.roots.scan_root)
                .to_path_buf();
            let Some(package) = manifest.package.as_ref() else {
                if manifest.workspace.is_some() {
                    output.candidates.extend(virtual_workspace_candidates(
                        context,
                        &manifest_path,
                        &package_directory,
                    ));
                }
                continue;
            };
            match context.invocation.intent {
                Intent::Run => {
                    let targets = executable_targets(context, &manifest, &package_directory);
                    if targets.iter().all(|target| target.example) {
                        output.diagnostics.push(Diagnostic {
                            detector: CARGO,
                            severity: crate::diagnostic::Severity::Info,
                            message: format!(
                                "crate `{}` has no executable binary targets; try build, test, or an example hint",
                                package.name
                            ),
                            source: Some(absolute_manifest.clone()),
                        });
                    }
                    output.candidates.extend(run_candidates(
                        package,
                        &manifest_path,
                        &package_directory,
                        workspace_root.as_deref(),
                        targets,
                    ));
                }
                Intent::Build | Intent::Test => {
                    if virtual_workspace_root.as_deref()
                        != Some(context.invocation.target.anchor_directory())
                    {
                        output.candidates.push(package_action_candidate(
                            context.invocation.intent,
                            package,
                            &manifest_path,
                            &package_directory,
                        ));
                    }
                }
            }
        }
        output
    }
}

fn cargo_manifests(context: &ScanCtx<'_>) -> (Vec<(PathBuf, CargoManifest)>, Vec<Diagnostic>) {
    let mut output = Vec::new();
    let mut diagnostics = Vec::new();
    let mut paths = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "Cargo.toml")
        })
        .map(|entry| entry.relative_path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    for path in paths {
        let absolute = context.roots.scan_root.join(&path);
        let contents = match context.index.manifests.read(&absolute) {
            Ok(contents) => contents,
            Err(error) => {
                diagnostics.push(Diagnostic::warning(
                    CARGO,
                    error.to_string(),
                    Some(absolute),
                ));
                continue;
            }
        };
        match toml::from_str::<CargoManifest>(&contents) {
            Ok(manifest) => output.push((path, manifest)),
            Err(error) => diagnostics.push(Diagnostic::warning(
                CARGO,
                format!("invalid Cargo.toml: {error}"),
                Some(absolute),
            )),
        }
    }
    (output, diagnostics)
}

fn executable_targets(
    context: &ScanCtx<'_>,
    manifest: &CargoManifest,
    package_directory: &Path,
) -> Vec<ExecutableTarget> {
    let mut targets = manifest
        .bin
        .iter()
        .filter_map(|target| {
            let path = target.path.clone().or_else(|| {
                target
                    .name
                    .as_ref()
                    .map(|name| PathBuf::from(format!("src/bin/{name}.rs")))
            })?;
            let name = target.name.clone().or_else(|| {
                path.file_stem()
                    .map(|value| value.to_string_lossy().into_owned())
            })?;
            Some(ExecutableTarget {
                name,
                path,
                example: false,
            })
        })
        .collect::<Vec<_>>();
    if package_directory.join("src/main.rs").is_file() {
        let name = manifest
            .package
            .as_ref()
            .map_or_else(|| "main".to_owned(), |package| package.name.clone());
        targets.push(ExecutableTarget {
            name,
            path: PathBuf::from("src/main.rs"),
            example: false,
        });
    }
    let package_relative = package_directory
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(Path::new(""));
    for entry in context.index.all_entries() {
        let Ok(relative_to_package) = entry.relative_path.strip_prefix(package_relative) else {
            continue;
        };
        let components = relative_to_package.components().collect::<Vec<_>>();
        let name = match components.as_slice() {
            [src, bin, file]
                if src.as_os_str() == "src"
                    && bin.as_os_str() == "bin"
                    && Path::new(file.as_os_str())
                        .extension()
                        .is_some_and(|ext| ext == "rs") =>
            {
                Path::new(file.as_os_str())
                    .file_stem()
                    .map(|value| value.to_string_lossy().into_owned())
            }
            [src, bin, directory, main]
                if src.as_os_str() == "src"
                    && bin.as_os_str() == "bin"
                    && main.as_os_str() == "main.rs" =>
            {
                Some(directory.as_os_str().to_string_lossy().into_owned())
            }
            _ => None,
        };
        if let Some(name) = name {
            targets.push(ExecutableTarget {
                name,
                path: relative_to_package.to_path_buf(),
                example: false,
            });
        }
    }
    let source_bins = package_directory.join("src/bin");
    if let Ok(entries) = std::fs::read_dir(&source_bins) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let (name, relative_path) =
                if path.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
                    (
                        path.file_stem()
                            .map(|value| value.to_string_lossy().into_owned()),
                        path.strip_prefix(package_directory)
                            .ok()
                            .map(Path::to_path_buf),
                    )
                } else if path.is_dir() && path.join("main.rs").is_file() {
                    (
                        path.file_name()
                            .map(|value| value.to_string_lossy().into_owned()),
                        path.join("main.rs")
                            .strip_prefix(package_directory)
                            .ok()
                            .map(Path::to_path_buf),
                    )
                } else {
                    (None, None)
                };
            if let (Some(name), Some(path)) = (name, relative_path) {
                targets.push(ExecutableTarget {
                    name,
                    path,
                    example: false,
                });
            }
        }
    }
    for target in &manifest.example {
        if let Some(name) = target.name.clone().or_else(|| {
            target
                .path
                .as_deref()
                .and_then(Path::file_stem)
                .map(|value| value.to_string_lossy().into_owned())
        }) {
            targets.push(ExecutableTarget {
                path: target
                    .path
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(format!("examples/{name}.rs"))),
                name,
                example: true,
            });
        }
    }
    let examples_directory = package_directory.join("examples");
    if let Ok(entries) = std::fs::read_dir(examples_directory) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                if let Some(name) = path.file_stem() {
                    targets.push(ExecutableTarget {
                        name: name.to_string_lossy().into_owned(),
                        path: PathBuf::from("examples").join(path.file_name().unwrap_or_default()),
                        example: true,
                    });
                }
            }
        }
    }
    targets.sort_by(|left, right| {
        left.example
            .cmp(&right.example)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.path.cmp(&right.path))
    });
    targets.dedup_by(|left, right| left.example == right.example && left.name == right.name);
    targets
}

fn run_candidates(
    package: &CargoPackage,
    manifest_path: &Path,
    package_directory: &Path,
    workspace_root: Option<&Path>,
    targets: Vec<ExecutableTarget>,
) -> Vec<Candidate> {
    let binary_count = targets.iter().filter(|target| !target.example).count();
    targets
        .into_iter()
        .map(|target| {
            let in_workspace = workspace_root.is_some_and(|root| root != package_directory);
            let cwd = workspace_root.unwrap_or(package_directory).to_path_buf();
            let mut args = vec![OsString::from("run")];
            if in_workspace {
                args.extend([OsString::from("-p"), OsString::from(&package.name)]);
            }
            let (base_points, selection) = if target.example {
                args.extend([OsString::from("--example"), OsString::from(&target.name)]);
                (25, SelectionPolicy::ExplicitHint)
            } else if binary_count == 1 {
                (95, SelectionPolicy::Automatic)
            } else {
                args.extend([OsString::from("--bin"), OsString::from(&target.name)]);
                let points = if package.default_run.as_deref() == Some(&target.name) {
                    95
                } else if target.name == package.name {
                    85
                } else {
                    75
                };
                (points, SelectionPolicy::Automatic)
            };
            let kind = if target.example { "example" } else { "bin" };
            let mut candidate = Candidate::new(
                format!("cargo:{}:{kind}:{}", package.name, target.name),
                CARGO,
                CARGO_SOURCE,
                Intent::Run,
                &target.name,
                "cargo",
                args,
                cwd,
                base_points,
                selection,
            );
            candidate.origin = if target.example {
                CandidateOrigin::Conventional
            } else {
                CandidateOrigin::Declared
            };
            candidate.layer = CommandLayer::DirectTarget;
            candidate.passthrough = crate::candidate::PassthroughStyle::DoubleDash;
            candidate.label = if target.example {
                format!("Cargo example `{}`", target.name)
            } else {
                format!("Cargo binary `{}`", target.name)
            };
            candidate.description = format!("Rust package `{}` {kind}", package.name);
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Manifest,
                reason: format!(
                    "Cargo package `{}` exposes {kind} `{}`",
                    package.name, target.name
                ),
                points: 0,
                source: Some(manifest_path.to_path_buf()),
            });
            if package.default_run.as_deref() == Some(&target.name) {
                candidate.evidence.push(Evidence {
                    kind: EvidenceKind::Rule,
                    reason: "matches package.default-run".to_owned(),
                    points: 10,
                    source: Some(manifest_path.to_path_buf()),
                });
            }
            candidate.search = SearchDocument {
                identities: vec![target.name],
                target_paths: vec![target.path],
                scopes: vec![package.name.clone()],
                tags: vec![
                    "rust".to_owned(),
                    "rs".to_owned(),
                    "cargo".to_owned(),
                    "crate".to_owned(),
                ],
                text: vec![candidate.description.clone()],
            };
            candidate
        })
        .collect()
}

fn package_action_candidate(
    intent: Intent,
    package: &CargoPackage,
    manifest_path: &Path,
    package_directory: &Path,
) -> Candidate {
    let action = intent.to_string();
    let mut candidate = Candidate::new(
        format!("cargo:{}:{action}", package.name),
        CARGO,
        CARGO_SOURCE,
        intent,
        &action,
        "cargo",
        vec![OsString::from(&action)],
        package_directory.to_path_buf(),
        95,
        SelectionPolicy::Automatic,
    );
    candidate.passthrough = crate::candidate::PassthroughStyle::Append;
    candidate.label = format!("Cargo {action} `{}`", package.name);
    candidate.description = format!("Cargo {action} for package `{}`", package.name);
    candidate.evidence.push(Evidence {
        kind: EvidenceKind::Manifest,
        reason: format!("Cargo.toml declares package `{}`", package.name),
        points: 0,
        source: Some(manifest_path.to_path_buf()),
    });
    candidate.search = SearchDocument {
        identities: vec![action],
        target_paths: vec![manifest_path.to_path_buf()],
        scopes: vec![package.name.clone()],
        tags: vec![
            "rust".to_owned(),
            "rs".to_owned(),
            "cargo".to_owned(),
            "crate".to_owned(),
        ],
        text: vec![candidate.description.clone()],
    };
    candidate
}

fn virtual_workspace_candidates(
    context: &ScanCtx<'_>,
    manifest_path: &Path,
    workspace_directory: &Path,
) -> Vec<Candidate> {
    if !matches!(context.invocation.intent, Intent::Build | Intent::Test) {
        return Vec::new();
    }
    if context.invocation.target.anchor_directory() != workspace_directory {
        return Vec::new();
    }
    let action = context.invocation.intent.to_string();
    let mut candidate = Candidate::new(
        format!("cargo:workspace:{action}"),
        CARGO,
        CARGO_SOURCE,
        context.invocation.intent,
        &action,
        "cargo",
        vec![OsString::from(&action), OsString::from("--workspace")],
        workspace_directory.to_path_buf(),
        95,
        SelectionPolicy::Automatic,
    );
    candidate.label = format!("Cargo {action} workspace");
    candidate.description = format!("Cargo {action} for the virtual workspace");
    candidate.evidence.push(Evidence {
        kind: EvidenceKind::Manifest,
        reason: "Cargo.toml declares a virtual workspace".to_owned(),
        points: 0,
        source: Some(manifest_path.to_path_buf()),
    });
    candidate.search = SearchDocument {
        identities: vec![action],
        target_paths: vec![manifest_path.to_path_buf()],
        scopes: vec!["workspace".to_owned()],
        tags: vec![
            "rust".to_owned(),
            "cargo".to_owned(),
            "workspace".to_owned(),
        ],
        text: vec![candidate.description.clone()],
    };
    vec![candidate]
}
