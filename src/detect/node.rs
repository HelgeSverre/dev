use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::candidate::{
    Candidate, CandidateOrigin, Evidence, EvidenceKind, Lifecycle, PassthroughStyle,
    SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::scan::IndexedFileType;

use super::{Detection, Detector, ScanCtx};

pub struct NodeDetector;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageManifest {
    name: Option<String>,
    package_manager: Option<String>,
    #[serde(default)]
    scripts: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    dependencies: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    dev_dependencies: BTreeMap<String, serde_json::Value>,
    bin: Option<serde_json::Value>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl PackageManager {
    fn program(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }

    fn script_args(self, script: &str) -> Vec<OsString> {
        vec![OsString::from("run"), OsString::from(script)]
    }

    fn passthrough(self) -> PassthroughStyle {
        match self {
            Self::Npm | Self::Pnpm => PassthroughStyle::NpmRun,
            Self::Yarn | Self::Bun => PassthroughStyle::Append,
        }
    }
}

impl Detector for NodeDetector {
    fn name(&self) -> &'static str {
        "node"
    }

    fn synonyms(&self) -> &'static [&'static str] {
        &[
            "javascript",
            "js",
            "typescript",
            "ts",
            "node",
            "npm",
            "pnpm",
            "yarn",
            "bun",
        ]
    }

    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut output = Detection::default();
        for manifest_path in package_manifests(context) {
            let absolute_manifest = context.roots.scan_root.join(&manifest_path);
            let contents = match context.index.manifests.read(&absolute_manifest) {
                Ok(contents) => contents,
                Err(error) => {
                    output.diagnostics.push(Diagnostic::warning(
                        self.name(),
                        error.to_string(),
                        Some(absolute_manifest),
                    ));
                    continue;
                }
            };
            let manifest = match serde_json::from_str::<PackageManifest>(&contents) {
                Ok(manifest) => manifest,
                Err(error) => {
                    output.diagnostics.push(Diagnostic::warning(
                        self.name(),
                        format!("invalid package.json: {error}"),
                        Some(absolute_manifest),
                    ));
                    continue;
                }
            };
            let package_directory = absolute_manifest
                .parent()
                .unwrap_or(&context.roots.scan_root)
                .to_path_buf();
            for (script, value) in &manifest.scripts {
                if !value.is_string() {
                    output.diagnostics.push(Diagnostic::warning(
                        self.name(),
                        format!("ignoring non-string package script `{script}`"),
                        Some(absolute_manifest.clone()),
                    ));
                }
            }
            let (manager, manager_reason) =
                package_manager(&manifest, &package_directory, &context.roots.scan_root);
            let framework = framework(&manifest, &package_directory);
            output.candidates.extend(script_candidates(
                context,
                &manifest,
                &manifest_path,
                &package_directory,
                manager,
                &manager_reason,
                framework,
                self.synonyms(),
            ));
            if context.invocation.intent == Intent::Run {
                output.candidates.extend(bin_candidates(
                    &manifest,
                    &manifest_path,
                    &package_directory,
                    self.synonyms(),
                ));
                output.candidates.extend(conventional_file_candidates(
                    context,
                    &manifest_path,
                    &package_directory,
                    self.synonyms(),
                ));
            }
            if let Some(framework) = framework {
                output.candidates.extend(framework_fallbacks(
                    context,
                    &manifest,
                    &manifest_path,
                    &package_directory,
                    manager,
                    framework,
                    self.synonyms(),
                ));
            }
        }
        output
    }
}

fn package_manifests(context: &ScanCtx<'_>) -> Vec<PathBuf> {
    let mut paths = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "package.json")
        })
        .map(|entry| entry.relative_path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

#[allow(clippy::too_many_arguments)]
fn script_candidates(
    context: &ScanCtx<'_>,
    manifest: &PackageManifest,
    manifest_path: &Path,
    package_directory: &Path,
    manager: PackageManager,
    manager_reason: &str,
    framework: Option<&'static str>,
    synonyms: &[&str],
) -> Vec<Candidate> {
    manifest
        .scripts
        .iter()
        .filter(|(_, value)| value.is_string())
        .filter_map(|(script, _)| {
            let canonical = canonical_script(context.invocation.intent, script);
            if canonical.is_none() && context.invocation.intent != Intent::Run {
                return None;
            }
            let (mut base_points, mut selection) = canonical
                .map_or((15, SelectionPolicy::ExplicitHint), |points| {
                    (points, SelectionPolicy::Automatic)
                });
            if framework == Some("next")
                && context.invocation.intent == Intent::Run
                && script == "start"
            {
                base_points = 55;
                selection = SelectionPolicy::ExplicitHint;
            }
            let detector = framework.filter(|_| {
                matches!(
                    (context.invocation.intent, script.as_str()),
                    (Intent::Run, "dev") | (Intent::Build, "build")
                )
            });
            let detector_name = detector.unwrap_or("node");
            let mut candidate = Candidate::new(
                format!(
                    "node:{}:script:{script}",
                    package_scope(manifest, package_directory)
                ),
                detector_name,
                context.invocation.intent,
                script,
                manager.program(),
                manager.script_args(script),
                package_directory.to_path_buf(),
                base_points,
                selection,
            );
            candidate.passthrough = manager.passthrough();
            candidate.lifecycle = if context.invocation.intent == Intent::Run {
                Lifecycle::LongRunning
            } else {
                Lifecycle::Finite
            };
            candidate.label = detector.map_or_else(
                || format!("{} script `{script}`", manager.program()),
                |framework| format!("{framework} {script}"),
            );
            candidate.description = format!(
                "Declared package script using {}; {manager_reason}",
                manager.program()
            );
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Manifest,
                reason: format!("package.json declares script `{script}`"),
                points: 0,
                source: Some(manifest_path.to_path_buf()),
            });
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Rule,
                reason: manager_reason.to_owned(),
                points: if manager_reason.starts_with("defaulted") {
                    -5
                } else {
                    0
                },
                source: None,
            });
            if let Some(framework) = detector {
                candidate.evidence.push(Evidence {
                    kind: EvidenceKind::Manifest,
                    reason: format!("package declares {framework}"),
                    points: 10,
                    source: Some(manifest_path.to_path_buf()),
                });
            }
            if framework == Some("next")
                && context.invocation.intent == Intent::Run
                && script == "start"
            {
                candidate
                    .description
                    .push_str("; requires a prior Next build");
            }
            candidate.search = SearchDocument {
                identities: vec![script.clone()],
                target_paths: vec![manifest_path.to_path_buf()],
                scopes: manifest
                    .name
                    .iter()
                    .cloned()
                    .chain(std::iter::once(package_scope(manifest, package_directory)))
                    .collect(),
                tags: synonyms
                    .iter()
                    .map(|value| (*value).to_owned())
                    .chain(detector.into_iter().map(str::to_owned))
                    .collect(),
                text: vec![candidate.description.clone()],
            };
            Some(candidate)
        })
        .collect()
}

fn canonical_script(intent: Intent, name: &str) -> Option<i32> {
    let names: &[(&str, i32)] = match intent {
        Intent::Run => &[("dev", 95), ("start", 85), ("serve", 75), ("watch", 65)],
        Intent::Build => &[("build", 95), ("compile", 80), ("bundle", 70)],
        Intent::Test => &[
            ("test", 95),
            ("test:unit", 85),
            ("spec", 75),
            ("vitest", 70),
            ("jest", 65),
        ],
    };
    names
        .iter()
        .find_map(|(candidate, points)| (*candidate == name).then_some(*points))
}

fn package_manager(
    manifest: &PackageManifest,
    directory: &Path,
    scan_root: &Path,
) -> (PackageManager, String) {
    if let Some(value) = manifest.package_manager.as_deref() {
        let name = value.split('@').next().unwrap_or(value);
        let manager = match name {
            "pnpm" => PackageManager::Pnpm,
            "yarn" => PackageManager::Yarn,
            "bun" => PackageManager::Bun,
            _ => PackageManager::Npm,
        };
        return (manager, format!("selected by packageManager `{value}`"));
    }
    let ancestors = directory
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(scan_root))
        .collect::<Vec<_>>();
    for (filename, manager) in [
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("package-lock.json", PackageManager::Npm),
    ] {
        if let Some(root) = ancestors
            .iter()
            .find(|ancestor| ancestor.join(filename).is_file())
        {
            return (
                manager,
                format!("selected by {}", root.join(filename).display()),
            );
        }
    }
    (
        PackageManager::Npm,
        "defaulted to npm because no package-manager marker was found".to_owned(),
    )
}

fn framework(manifest: &PackageManifest, directory: &Path) -> Option<&'static str> {
    let has_dependency = |name: &str| {
        manifest.dependencies.contains_key(name) || manifest.dev_dependencies.contains_key(name)
    };
    let has_config = |prefix: &str| {
        std::fs::read_dir(directory).ok().is_some_and(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        })
    };
    if has_dependency("next") || has_config("next.config.") {
        Some("next")
    } else if has_dependency("vite") || has_config("vite.config.") {
        Some("vite")
    } else {
        None
    }
}

fn bin_candidates(
    manifest: &PackageManifest,
    manifest_path: &Path,
    package_directory: &Path,
    synonyms: &[&str],
) -> Vec<Candidate> {
    let bins = match &manifest.bin {
        Some(serde_json::Value::String(path)) => vec![(
            manifest.name.clone().unwrap_or_else(|| "bin".to_owned()),
            path.clone(),
        )],
        Some(serde_json::Value::Object(values)) => values
            .iter()
            .filter_map(|(name, value)| value.as_str().map(|path| (name.clone(), path.to_owned())))
            .collect(),
        _ => Vec::new(),
    };
    bins.into_iter()
        .map(|(name, path)| {
            let mut candidate = Candidate::new(
                format!(
                    "node:{}:bin:{name}",
                    package_scope(manifest, package_directory)
                ),
                "node",
                Intent::Run,
                &name,
                "node",
                vec![OsString::from(&path)],
                package_directory.to_path_buf(),
                35,
                SelectionPolicy::ExplicitHint,
            );
            candidate.label = format!("Node binary `{name}`");
            candidate.description = "Explicit package.json bin entry".to_owned();
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Manifest,
                reason: format!("package.json declares bin `{name}`"),
                points: 0,
                source: Some(manifest_path.to_path_buf()),
            });
            candidate.search = SearchDocument {
                identities: vec![name],
                target_paths: vec![PathBuf::from(path)],
                scopes: manifest.name.iter().cloned().collect(),
                tags: synonyms.iter().map(|value| (*value).to_owned()).collect(),
                text: vec![candidate.description.clone()],
            };
            candidate
        })
        .collect()
}

fn conventional_file_candidates(
    context: &ScanCtx<'_>,
    manifest_path: &Path,
    package_directory: &Path,
    synonyms: &[&str],
) -> Vec<Candidate> {
    ["server.js", "app.js", "index.js"]
        .into_iter()
        .filter(|filename| package_directory.join(filename).is_file())
        .map(|filename| {
            let mut candidate = Candidate::new(
                format!("node:{}:file:{filename}", package_directory.display()),
                "node",
                Intent::Run,
                filename,
                "node",
                vec![OsString::from(filename)],
                package_directory.to_path_buf(),
                25,
                if context.invocation.target.path() == package_directory.join(filename) {
                    SelectionPolicy::Automatic
                } else {
                    SelectionPolicy::ExplicitHint
                },
            );
            candidate.origin = CandidateOrigin::Conventional;
            candidate.lifecycle = Lifecycle::LongRunning;
            candidate.label = format!("Node file `{filename}`");
            candidate.description = "Conventional Node entry file".to_owned();
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Convention,
                reason: format!("found conventional entry file `{filename}`"),
                points: 0,
                source: Some(manifest_path.with_file_name(filename)),
            });
            candidate.search = SearchDocument {
                identities: vec![
                    filename.trim_end_matches(".js").to_owned(),
                    filename.to_owned(),
                ],
                target_paths: vec![PathBuf::from(filename)],
                scopes: Vec::new(),
                tags: synonyms.iter().map(|value| (*value).to_owned()).collect(),
                text: vec![candidate.description.clone()],
            };
            candidate
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn framework_fallbacks(
    context: &ScanCtx<'_>,
    manifest: &PackageManifest,
    manifest_path: &Path,
    package_directory: &Path,
    manager: PackageManager,
    framework: &'static str,
    synonyms: &[&str],
) -> Vec<Candidate> {
    let action = match (framework, context.invocation.intent) {
        ("vite", Intent::Run) if !manifest.scripts.contains_key("dev") => {
            Some(("dev", vec!["vite"]))
        }
        ("vite", Intent::Build) if !manifest.scripts.contains_key("build") => {
            Some(("build", vec!["vite", "build"]))
        }
        ("next", Intent::Run) if !manifest.scripts.contains_key("dev") => {
            Some(("dev", vec!["next", "dev"]))
        }
        ("next", Intent::Build) if !manifest.scripts.contains_key("build") => {
            Some(("build", vec!["next", "build"]))
        }
        _ => None,
    };
    let Some((name, command)) = action else {
        return Vec::new();
    };
    let executable = package_directory
        .join("node_modules")
        .join(".bin")
        .join(command[0]);
    let mut candidate = Candidate::new(
        format!(
            "{framework}:{}:{name}",
            package_scope(manifest, package_directory)
        ),
        framework,
        context.invocation.intent,
        name,
        executable.as_os_str(),
        command[1..].iter().map(OsString::from).collect(),
        package_directory.to_path_buf(),
        80,
        SelectionPolicy::Automatic,
    );
    candidate.lifecycle = if context.invocation.intent == Intent::Run {
        Lifecycle::LongRunning
    } else {
        Lifecycle::Finite
    };
    candidate.label = format!("{framework} {name}");
    candidate.description = format!("Verified project-local {framework} binary");
    candidate.evidence.push(Evidence {
        kind: EvidenceKind::Manifest,
        reason: format!("package declares {framework} without a canonical `{name}` script"),
        points: 10,
        source: Some(manifest_path.to_path_buf()),
    });
    candidate.search = SearchDocument {
        identities: vec![name.to_owned(), framework.to_owned()],
        target_paths: vec![manifest_path.to_path_buf()],
        scopes: manifest.name.iter().cloned().collect(),
        tags: synonyms
            .iter()
            .map(|value| (*value).to_owned())
            .chain(std::iter::once(framework.to_owned()))
            .collect(),
        text: vec![candidate.description.clone(), manager.program().to_owned()],
    };
    vec![candidate]
}

fn package_scope(manifest: &PackageManifest, directory: &Path) -> String {
    manifest
        .name
        .clone()
        .or_else(|| {
            directory
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| ".".to_owned())
}
