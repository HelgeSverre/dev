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
use crate::scan::{IndexEntry, IndexedFileType};

use super::{Detection, Detector, ScanCtx, TargetBinder};

pub struct NodeDetector;
pub(super) struct NodeTestBinder;

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

    fn local_binary_args(self, binary: &str, arguments: &[&str]) -> Vec<OsString> {
        let mut output = match self {
            Self::Npm => vec![
                OsString::from("exec"),
                OsString::from("--offline"),
                OsString::from("--"),
                OsString::from(binary),
            ],
            Self::Pnpm => vec![OsString::from("exec"), OsString::from(binary)],
            Self::Yarn => vec![OsString::from("run"), OsString::from(binary)],
            Self::Bun => vec![
                OsString::from("x"),
                OsString::from("--no-install"),
                OsString::from(binary),
            ],
        };
        output.extend(arguments.iter().map(OsString::from));
        output
    }

    fn apply_local_exec_safety(self, candidate: &mut Candidate) {
        if self == Self::Pnpm {
            candidate.env.insert(
                OsString::from("npm_config_verify_deps_before_run"),
                OsString::from("false"),
            );
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
            let detector = framework.filter(|framework| {
                matches!(
                    (*framework, context.invocation.intent, script.as_str()),
                    ("vite", Intent::Run, "dev" | "preview")
                        | ("vite", Intent::Build, "build")
                        | ("next", Intent::Run, "dev" | "start")
                        | ("next", Intent::Build, "build")
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
            if framework == Some("vite")
                && context.invocation.intent == Intent::Run
                && script == "preview"
            {
                candidate
                    .description
                    .push_str("; requires a prior Vite build");
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
                scopes: vec![package_scope(manifest, package_directory)],
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
                scopes: vec![package_scope_from_directory(package_directory)],
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
    let mut actions = Vec::<(&str, Vec<&str>, i32, SelectionPolicy, bool)>::new();
    match (framework, context.invocation.intent) {
        ("vite", Intent::Run) => {
            if !manifest.scripts.contains_key("dev") {
                actions.push(("dev", Vec::new(), 80, SelectionPolicy::Automatic, false));
            }
            if !manifest.scripts.contains_key("preview") {
                actions.push((
                    "preview",
                    vec!["preview"],
                    25,
                    SelectionPolicy::ExplicitHint,
                    true,
                ));
            }
        }
        ("vite", Intent::Build) if !manifest.scripts.contains_key("build") => {
            actions.push((
                "build",
                vec!["build"],
                80,
                SelectionPolicy::Automatic,
                false,
            ));
        }
        ("next", Intent::Run) => {
            if !manifest.scripts.contains_key("dev") {
                actions.push(("dev", vec!["dev"], 80, SelectionPolicy::Automatic, false));
            }
            if !manifest.scripts.contains_key("start") {
                actions.push((
                    "start",
                    vec!["start"],
                    25,
                    SelectionPolicy::ExplicitHint,
                    true,
                ));
            }
        }
        ("next", Intent::Build) if !manifest.scripts.contains_key("build") => {
            actions.push((
                "build",
                vec!["build"],
                80,
                SelectionPolicy::Automatic,
                false,
            ));
        }
        _ => {}
    }

    let local_binary_available =
        project_local_binary_exists(package_directory, &context.roots.scan_root, framework);
    actions
        .into_iter()
        .map(|(name, arguments, base_points, selection, requires_build)| {
            let mut candidate = Candidate::new(
                format!(
                    "{framework}:{}:{name}",
                    package_scope(manifest, package_directory)
                ),
                framework,
                context.invocation.intent,
                name,
                manager.program(),
                manager.local_binary_args(framework, &arguments),
                package_directory.to_path_buf(),
                base_points,
                selection,
            );
            manager.apply_local_exec_safety(&mut candidate);
            if !local_binary_available {
                candidate.availability = crate::candidate::Availability::UnsupportedHost {
                    reason: format!(
                        "project-local {framework} binary is not installed; dev will not download it"
                    ),
                };
            }
            candidate.lifecycle = if context.invocation.intent == Intent::Run {
                Lifecycle::LongRunning
            } else {
                Lifecycle::Finite
            };
            candidate.label = format!("{framework} {name}");
            candidate.description = format!(
                "Project-local {framework} through {} without installation",
                manager.program()
            );
            if requires_build {
                candidate
                    .description
                    .push_str(&format!(
                        "; requires a prior {} build",
                        framework_display(framework)
                    ));
            }
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Manifest,
                reason: format!(
                    "package declares {framework} without a canonical `{name}` script"
                ),
                points: 10,
                source: Some(manifest_path.to_path_buf()),
            });
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Rule,
                reason: format!(
                    "{} uses its local-only binary execution mode",
                    manager.program()
                ),
                points: 0,
                source: None,
            });
            candidate.search = SearchDocument {
                identities: vec![name.to_owned(), framework.to_owned()],
                target_paths: vec![manifest_path.to_path_buf()],
                scopes: vec![package_scope(manifest, package_directory)],
                tags: synonyms
                    .iter()
                    .map(|value| (*value).to_owned())
                    .chain(std::iter::once(framework.to_owned()))
                    .collect(),
                text: vec![candidate.description.clone(), manager.program().to_owned()],
            };
            candidate
        })
        .collect()
}

fn project_local_binary_exists(package_directory: &Path, scan_root: &Path, binary: &str) -> bool {
    package_directory
        .ancestors()
        .take_while(|directory| directory.starts_with(scan_root))
        .any(|directory| {
            let bin_directory = directory.join("node_modules").join(".bin");
            ["", ".cmd", ".exe", ".bat", ".com"]
                .into_iter()
                .any(|extension| {
                    project_binary_file(&bin_directory.join(format!("{binary}{extension}")))
                })
                || directory.join(".pnp.cjs").is_file()
        })
}

#[cfg(unix)]
fn project_binary_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn project_binary_file(path: &Path) -> bool {
    path.is_file()
}

fn framework_display(framework: &str) -> &str {
    match framework {
        "vite" => "Vite",
        "next" => "Next",
        other => other,
    }
}

fn package_scope(manifest: &PackageManifest, directory: &Path) -> String {
    manifest
        .name
        .clone()
        .unwrap_or_else(|| package_scope_from_directory(directory))
}

fn package_scope_from_directory(directory: &Path) -> String {
    directory.file_name().map_or_else(
        || ".".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl TargetBinder for NodeTestBinder {
    fn supports(&self, base: &Candidate, target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
        if base.intent != Intent::Test
            || !base.action_key.contains(":script:")
            || !is_javascript_test(&target.relative_path)
        {
            return false;
        }
        let absolute = context.roots.scan_root.join(&target.relative_path);
        absolute.starts_with(&base.cwd)
            && closest_node_package(context, &absolute).as_deref() == Some(base.cwd.as_path())
    }

    fn bind(
        &self,
        base: &Candidate,
        target: &IndexEntry,
        context: &ScanCtx<'_>,
    ) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        let relative = absolute.strip_prefix(&base.cwd).ok()?.to_path_buf();
        let mut candidate = base.clone();
        candidate.args = candidate.command_with_passthrough(&[relative.as_os_str().to_owned()]);
        candidate.passthrough = PassthroughStyle::Append;
        candidate.action_key = format!(
            "{}:target:{}",
            base.action_key,
            relative.to_string_lossy().replace(['/', '\\'], ":")
        );
        let identity = relative.file_stem().map_or_else(
            || relative.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        candidate.search.identities.push(identity);
        candidate.search.target_paths.push(relative.clone());
        candidate.evidence.push(Evidence {
            kind: EvidenceKind::Rule,
            reason: format!(
                "bound package test script to {}",
                target.relative_path.display()
            ),
            points: 20,
            source: Some(target.relative_path.clone()),
        });
        candidate.label = format!("{} — {}", base.label, relative.display());
        candidate.description =
            "Declared package test script bound to a matching test file".to_owned();
        Some(candidate)
    }
}

fn closest_node_package(context: &ScanCtx<'_>, target: &Path) -> Option<PathBuf> {
    context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "package.json")
        })
        .filter_map(|entry| {
            context
                .roots
                .scan_root
                .join(&entry.relative_path)
                .parent()
                .map(Path::to_path_buf)
        })
        .filter(|directory| target.starts_with(directory))
        .max_by_key(|directory| directory.components().count())
}

fn is_javascript_test(path: &Path) -> bool {
    let supported_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx"));
    if !supported_extension {
        return false;
    }
    let in_test_directory = path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some("test" | "tests" | "spec" | "__tests__")
        )
    });
    let filename = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().to_lowercase());
    in_test_directory
        || filename.contains(".test.")
        || filename.contains(".spec.")
        || filename.contains("_test.")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn framework_binary_commands_use_each_managers_local_only_mode() {
        let arguments = ["preview", "--host"];
        assert_eq!(
            PackageManager::Npm.local_binary_args("vite", &arguments),
            ["exec", "--offline", "--", "vite", "preview", "--host"].map(OsString::from)
        );
        assert_eq!(
            PackageManager::Pnpm.local_binary_args("vite", &arguments),
            ["exec", "vite", "preview", "--host"].map(OsString::from)
        );
        assert_eq!(
            PackageManager::Yarn.local_binary_args("vite", &arguments),
            ["run", "vite", "preview", "--host"].map(OsString::from)
        );
        assert_eq!(
            PackageManager::Bun.local_binary_args("vite", &arguments),
            ["x", "--no-install", "vite", "preview", "--host"].map(OsString::from)
        );
    }

    #[test]
    fn pnpm_local_exec_disables_dependency_auto_install_preflight() {
        let mut candidate = Candidate::new(
            "vite:test",
            "vite",
            Intent::Run,
            "dev",
            "pnpm",
            Vec::new(),
            PathBuf::from("/tmp"),
            80,
            SelectionPolicy::Automatic,
        );
        PackageManager::Pnpm.apply_local_exec_safety(&mut candidate);
        assert_eq!(
            candidate
                .env
                .get(&OsString::from("npm_config_verify_deps_before_run")),
            Some(&OsString::from("false"))
        );
    }
}
