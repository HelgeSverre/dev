use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::candidate::{
    Availability, Candidate, CandidateOrigin, Evidence, EvidenceKind, Lifecycle, PassthroughStyle,
    SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{
    RootClassification, ScanContribution, ToolId, WorkspaceContributor, BUN_TOOL, NEXT_SOURCE,
    NODE, NODE_SOURCE, NODE_TOOL, NPM_TOOL, PNPM_TOOL, SVELTEKIT_SOURCE, VITE_SOURCE, YARN_TOOL,
};
use crate::scan::{DiscoveryFiles, IndexEntry, IndexedFileType};

use super::{CandidateBuilder, Detection, Detector, ScanCtx, TargetBinder};

pub struct NodeDetector;
pub struct NodeWorkspaceContributor;
pub(crate) struct NodeTestBinder;

impl WorkspaceContributor for NodeWorkspaceContributor {
    fn classify_root(&self, marker: &Path, files: &DiscoveryFiles) -> RootClassification {
        if files
            .read(marker)
            .ok()
            .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok())
            .is_none()
        {
            return RootClassification::Neither;
        }
        let root = marker.parent().unwrap_or(Path::new("."));
        if self.scan_contribution(root, files).includes.is_empty() {
            RootClassification::Package
        } else {
            RootClassification::PackageAndWorkspace
        }
    }

    fn scan_contribution(&self, root: &Path, files: &DiscoveryFiles) -> ScanContribution {
        let mut contribution = ScanContribution::default();
        if let Ok(contents) = files.read(&root.join("package.json")) {
            if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&contents) {
                let workspaces = manifest.get("workspaces");
                let values = workspaces
                    .and_then(serde_json::Value::as_array)
                    .or_else(|| workspaces?.get("packages")?.as_array());
                if let Some(values) = values {
                    contribution
                        .includes
                        .extend(values.iter().filter_map(|value| {
                            value
                                .as_str()
                                .map(|pattern| append_workspace_manifest(pattern, "package.json"))
                        }));
                }
            }
        }
        if let Ok(contents) = files.read(&root.join("pnpm-workspace.yaml")) {
            if let Ok(manifest) = serde_yaml::from_str::<serde_yaml::Value>(&contents) {
                if let Some(packages) = manifest
                    .get("packages")
                    .and_then(serde_yaml::Value::as_sequence)
                {
                    for pattern in packages.iter().filter_map(serde_yaml::Value::as_str) {
                        if let Some(excluded) = pattern.strip_prefix('!') {
                            contribution
                                .excludes
                                .push(append_workspace_manifest(excluded, "package.json"));
                        } else {
                            contribution
                                .includes
                                .push(append_workspace_manifest(pattern, "package.json"));
                        }
                    }
                }
            }
        }
        contribution.includes.sort();
        contribution.includes.dedup();
        contribution.excludes.sort();
        contribution.excludes.dedup();
        contribution
    }
}

fn append_workspace_manifest(pattern: &str, manifest: &str) -> String {
    format!("{}/{manifest}", pattern.trim_end_matches(['/', '\\']))
}

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

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkspaceMember {
    root: PathBuf,
    relative_path: PathBuf,
    name: Option<String>,
}

impl WorkspaceMember {
    fn selector(&self) -> OsString {
        self.name.as_ref().map(OsString::from).unwrap_or_else(|| {
            let mut selector = PathBuf::from(".");
            for component in self.relative_path.components() {
                selector.push(component);
            }
            selector.into_os_string()
        })
    }

    fn scope(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.relative_path
                .to_string_lossy()
                .replace(['/', '\\'], ":")
        })
    }

    fn evidence(&self, manager: PackageManager, manifest_path: &Path) -> Evidence {
        let member = self.name.as_ref().map_or_else(
            || {
                format!(
                    "unnamed workspace member `{}`",
                    self.relative_path.display()
                )
            },
            |name| {
                format!(
                    "workspace member `{name}` at `{}`",
                    self.relative_path.display()
                )
            },
        );
        Evidence {
            kind: EvidenceKind::Rule,
            reason: format!(
                "{member} selected with {}",
                manager.workspace_selector_description(self.name.is_some())
            ),
            points: 0,
            source: Some(manifest_path.to_path_buf()),
        }
    }
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

    fn tool(self) -> ToolId {
        match self {
            Self::Npm => NPM_TOOL,
            Self::Pnpm => PNPM_TOOL,
            Self::Yarn => YARN_TOOL,
            Self::Bun => BUN_TOOL,
        }
    }

    fn script_args(self, script: &str, workspace: Option<&WorkspaceMember>) -> Vec<OsString> {
        let Some(workspace) = workspace else {
            return vec![OsString::from("run"), OsString::from(script)];
        };
        let selector = workspace.selector();
        match (self, workspace.name.is_some()) {
            (Self::Npm, _) => vec![
                OsString::from("run"),
                OsString::from(script),
                OsString::from("--workspace"),
                selector,
            ],
            (Self::Pnpm, _) => vec![
                OsString::from("--filter"),
                selector,
                OsString::from("run"),
                OsString::from(script),
            ],
            (Self::Yarn, true) => vec![
                OsString::from("workspace"),
                selector,
                OsString::from("run"),
                OsString::from(script),
            ],
            (Self::Yarn, false) => vec![
                OsString::from("--cwd"),
                selector,
                OsString::from("run"),
                OsString::from(script),
            ],
            (Self::Bun, _) => vec![
                OsString::from("run"),
                OsString::from("--filter"),
                selector,
                OsString::from(script),
            ],
        }
    }

    fn workspace_selector_description(self, named: bool) -> &'static str {
        match (self, named) {
            (Self::Npm, _) => "npm --workspace",
            (Self::Pnpm, _) => "pnpm --filter",
            (Self::Yarn, true) => "yarn workspace",
            (Self::Yarn, false) => "yarn --cwd",
            (Self::Bun, _) => "bun --filter",
        }
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

    fn local_exec_env(self) -> BTreeMap<OsString, OsString> {
        let mut env = BTreeMap::new();
        if self == Self::Pnpm {
            env.insert(
                OsString::from("npm_config_verify_deps_before_run"),
                OsString::from("false"),
            );
        }
        env
    }
}

impl Detector for NodeDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut output = Detection::default();
        for manifest_path in package_manifests(context) {
            let absolute_manifest = context.roots.scan_root.join(&manifest_path);
            let contents = match context.index.manifests.read(&absolute_manifest) {
                Ok(contents) => contents,
                Err(error) => {
                    output.diagnostics.push(Diagnostic::warning(
                        NODE,
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
                        NODE,
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
                        NODE,
                        format!("ignoring non-string package script `{script}`"),
                        Some(absolute_manifest.clone()),
                    ));
                } else if !safe_script_name(script) {
                    output.diagnostics.push(Diagnostic::warning(
                        NODE,
                        format!(
                            "ignoring package script `{script}` because the runner would parse it as an option"
                        ),
                        Some(absolute_manifest.clone()),
                    ));
                }
            }
            let (manager, manager_reason) = package_manager(
                &manifest,
                &package_directory,
                &context.roots.scan_root,
                &context.index.manifests,
            );
            let framework = framework(&manifest, &package_directory);
            output.candidates.extend(script_candidates(
                context,
                &manifest,
                &manifest_path,
                &package_directory,
                manager,
                &manager_reason,
                framework,
                crate::registry::synonyms(NODE),
            ));
            if context.invocation.intent == Intent::Run {
                output.candidates.extend(bin_candidates(
                    &manifest,
                    &manifest_path,
                    &package_directory,
                    crate::registry::synonyms(NODE),
                ));
                output.candidates.extend(conventional_file_candidates(
                    context,
                    &manifest_path,
                    &package_directory,
                    crate::registry::synonyms(NODE),
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
                    crate::registry::synonyms(NODE),
                ));
            }
            if manager == PackageManager::Bun {
                output.candidates.extend(bun_native_candidates(
                    context,
                    &manifest,
                    &package_directory,
                    &manifest_path,
                ));
            }
        }
        output
    }
}

fn package_manifests(context: &ScanCtx<'_>) -> Vec<PathBuf> {
    let node_workspace_root = context
        .roots
        .workspace_root
        .as_ref()
        .filter(|root| is_node_workspace(root, &context.index.manifests));
    let mut paths = context
        .index
        .all_entries()
        .filter(|entry| {
            if entry.file_type != IndexedFileType::File
                || !entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "package.json")
            {
                return false;
            }
            let Some(workspace_root) = node_workspace_root else {
                return true;
            };
            let absolute_manifest = context.roots.scan_root.join(&entry.relative_path);
            let package_directory = absolute_manifest.parent();
            if package_directory == Some(workspace_root.as_path())
                || package_directory == context.roots.package_root.as_deref()
            {
                return true;
            }
            absolute_manifest
                .strip_prefix(workspace_root)
                .is_ok_and(|relative| {
                    crate::registry::workspace_contains_manifest(
                        NODE,
                        workspace_root,
                        relative,
                        &context.index.manifests,
                    )
                })
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
    let workspace = workspace_member(context, manifest, manifest_path, package_directory);
    let action_scope = workspace.as_ref().map_or_else(
        || package_scope(manifest, package_directory),
        WorkspaceMember::scope,
    );
    manifest
        .scripts
        .iter()
        .filter(|(script, value)| value.is_string() && safe_script_name(script))
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
                        | ("sveltekit", Intent::Run, "dev" | "preview")
                        | ("sveltekit", Intent::Build, "build")
                )
            });
            let source = match detector {
                Some("vite") => VITE_SOURCE,
                Some("next") => NEXT_SOURCE,
                Some("sveltekit") => SVELTEKIT_SOURCE,
                _ => NODE_SOURCE,
            };
            let cwd = workspace.as_ref().map_or_else(
                || package_directory.to_path_buf(),
                |member| member.root.clone(),
            );
            let lifecycle = if context.invocation.intent == Intent::Run {
                Lifecycle::LongRunning
            } else {
                Lifecycle::Finite
            };
            let label = detector.map_or_else(
                || format!("{} script `{script}`", manager.program()),
                |framework| format!("{framework} {script}"),
            );
            let mut description = format!(
                "Declared package script using {}; {manager_reason}",
                manager.program()
            );
            if framework == Some("next")
                && context.invocation.intent == Intent::Run
                && script == "start"
            {
                description.push_str("; requires a prior Next build");
            }
            if framework == Some("vite")
                && context.invocation.intent == Intent::Run
                && script == "preview"
            {
                description.push_str("; requires a prior Vite build");
            }
            if framework == Some("sveltekit")
                && context.invocation.intent == Intent::Run
                && script == "preview"
            {
                description.push_str("; requires a prior SvelteKit build");
            }
            let mut evidence = vec![Evidence {
                kind: EvidenceKind::Manifest,
                reason: format!("package.json declares script `{script}`"),
                points: 0,
                source: Some(manifest_path.to_path_buf()),
            }];
            if let Some(workspace) = &workspace {
                evidence.push(workspace.evidence(manager, manifest_path));
            }
            evidence.push(Evidence {
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
                evidence.push(Evidence {
                    kind: EvidenceKind::Manifest,
                    reason: format!("package declares {framework}"),
                    points: 10,
                    source: Some(manifest_path.to_path_buf()),
                });
            }
            let candidate = CandidateBuilder::ecosystem_task(
                source,
                context.invocation.intent,
                package_directory.to_path_buf(),
                script,
            )
            .action_key(format!("node:{}:script:{script}", action_scope))
            .tool(manager.tool())
            .args(manager.script_args(script, workspace.as_ref()))
            .cwd(cwd)
            .selection(selection)
            .base_points(base_points)
            .passthrough(manager.passthrough())
            .lifecycle(lifecycle)
            .label(label)
            .description(&description)
            .evidence_all(evidence)
            .search(SearchDocument {
                identities: vec![script.clone()],
                target_paths: vec![manifest_path.to_path_buf()],
                scopes: manifest
                    .name
                    .iter()
                    .cloned()
                    .chain(std::iter::once(action_scope.clone()))
                    .chain(
                        workspace
                            .iter()
                            .map(|member| member.relative_path.to_string_lossy().into_owned()),
                    )
                    .collect(),
                tags: synonyms
                    .iter()
                    .map(|value| (*value).to_owned())
                    .chain(detector.into_iter().map(str::to_owned))
                    .collect(),
                text: vec![description],
            })
            .build()
            .expect("Node script candidate registration is valid");
            Some(candidate)
        })
        .collect()
}

fn safe_script_name(name: &str) -> bool {
    !name.starts_with('-')
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
    files: &DiscoveryFiles,
) -> (PackageManager, String) {
    if let Some(value) = manifest.package_manager.as_deref() {
        return (
            package_manager_name(value),
            format!("selected by packageManager `{value}`"),
        );
    }
    let manifest_relative = directory
        .join("package.json")
        .strip_prefix(scan_root)
        .ok()
        .map(Path::to_path_buf);
    let manager_root = if directory != scan_root
        && is_node_workspace(scan_root, files)
        && !manifest_relative.as_deref().is_some_and(|relative| {
            crate::registry::workspace_contains_manifest(NODE, scan_root, relative, files)
        }) {
        directory
    } else {
        scan_root
    };
    for ancestor in directory
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|ancestor| ancestor.starts_with(manager_root))
    {
        let manifest_path = ancestor.join("package.json");
        let Some(value) = files
            .read(&manifest_path)
            .ok()
            .and_then(|contents| serde_json::from_str::<PackageManifest>(&contents).ok())
            .and_then(|manifest| manifest.package_manager)
        else {
            continue;
        };
        return (
            package_manager_name(&value),
            format!(
                "selected by packageManager `{value}` in {}",
                manifest_path.display()
            ),
        );
    }
    let ancestors = directory
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(manager_root))
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

fn package_manager_name(value: &str) -> PackageManager {
    match value.split('@').next().unwrap_or(value) {
        "pnpm" => PackageManager::Pnpm,
        "yarn" => PackageManager::Yarn,
        "bun" => PackageManager::Bun,
        _ => PackageManager::Npm,
    }
}

fn workspace_member(
    context: &ScanCtx<'_>,
    manifest: &PackageManifest,
    manifest_path: &Path,
    package_directory: &Path,
) -> Option<WorkspaceMember> {
    let root = context.roots.workspace_root.as_ref()?;
    if package_directory == root {
        return None;
    }
    let absolute_manifest = context.roots.scan_root.join(manifest_path);
    let relative_manifest = absolute_manifest.strip_prefix(root).ok()?;
    if !crate::registry::workspace_contains_manifest(
        NODE,
        root,
        relative_manifest,
        &context.index.manifests,
    ) {
        return None;
    }
    let relative_path = package_directory.strip_prefix(root).ok()?.to_path_buf();
    (!relative_path.as_os_str().is_empty()).then(|| WorkspaceMember {
        root: root.clone(),
        relative_path,
        name: manifest.name.clone(),
    })
}

fn is_node_workspace(root: &Path, files: &DiscoveryFiles) -> bool {
    crate::registry::workspace(NODE)
        .is_some_and(|workspace| !workspace.scan_contribution(root, files).includes.is_empty())
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
    } else if has_dependency("@sveltejs/kit") || has_config("svelte.config.") {
        Some("sveltekit")
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
            let description = "Explicit package.json bin entry".to_owned();
            CandidateBuilder::direct_target(
                NODE_SOURCE,
                Intent::Run,
                package_directory.to_path_buf(),
                &name,
            )
            .action_key(format!(
                "node:{}:bin:{name}",
                package_scope(manifest, package_directory)
            ))
            .tool(NODE_TOOL)
            .args([OsString::from(&path)])
            .cwd(package_directory.to_path_buf())
            .selection(SelectionPolicy::ExplicitHint)
            .base_points(35)
            .label(format!("Node binary `{name}`"))
            .description(&description)
            .evidence(Evidence {
                kind: EvidenceKind::Manifest,
                reason: format!("package.json declares bin `{name}`"),
                points: 0,
                source: Some(manifest_path.to_path_buf()),
            })
            .search(SearchDocument {
                identities: vec![name],
                target_paths: vec![PathBuf::from(path)],
                scopes: vec![package_scope(manifest, package_directory)],
                tags: synonyms.iter().map(|value| (*value).to_owned()).collect(),
                text: vec![description],
            })
            .build()
            .expect("Node bin candidate registration is valid")
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
            let description = "Conventional Node entry file".to_owned();
            CandidateBuilder::direct_target(
                NODE_SOURCE,
                Intent::Run,
                package_directory.to_path_buf(),
                filename,
            )
            .action_key(format!(
                "node:{}:file:{filename}",
                package_directory.display()
            ))
            .tool(NODE_TOOL)
            .args([OsString::from(filename)])
            .cwd(package_directory.to_path_buf())
            .selection(
                if context.invocation.target.path() == package_directory.join(filename) {
                    SelectionPolicy::Automatic
                } else {
                    SelectionPolicy::ExplicitHint
                },
            )
            .base_points(25)
            .origin(CandidateOrigin::Conventional)
            .lifecycle(Lifecycle::LongRunning)
            .label(format!("Node file `{filename}`"))
            .description(&description)
            .evidence(Evidence {
                kind: EvidenceKind::Convention,
                reason: format!("found conventional entry file `{filename}`"),
                points: 0,
                source: Some(manifest_path.with_file_name(filename)),
            })
            .search(SearchDocument {
                identities: vec![
                    filename.trim_end_matches(".js").to_owned(),
                    filename.to_owned(),
                ],
                target_paths: vec![PathBuf::from(filename)],
                scopes: vec![package_scope_from_directory(package_directory)],
                tags: synonyms.iter().map(|value| (*value).to_owned()).collect(),
                text: vec![description],
            })
            .build()
            .expect("Node file candidate registration is valid")
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
        ("sveltekit", Intent::Run) => {
            if !manifest.scripts.contains_key("dev") {
                actions.push(("dev", vec!["dev"], 80, SelectionPolicy::Automatic, false));
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
        ("sveltekit", Intent::Build) if !manifest.scripts.contains_key("build") => {
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

    let local_binary = project_local_binary(
        package_directory,
        &context.roots.scan_root,
        framework_binary_name(framework),
    );
    actions
        .into_iter()
        .map(|(name, arguments, base_points, selection, requires_build)| {
            let source = match framework {
                "vite" => VITE_SOURCE,
                "next" => NEXT_SOURCE,
                "sveltekit" => SVELTEKIT_SOURCE,
                _ => NODE_SOURCE,
            };
            let mut description = format!(
                "Project-local {framework} through {} without installation",
                manager.program()
            );
            if requires_build {
                description.push_str(&format!(
                    "; requires a prior {} build",
                    framework_display(framework)
                ));
            }
            let availability = local_binary.is_none().then(|| Availability::UnsupportedHost {
                    reason: format!(
                        "project-local {framework} binary is not installed; dev will not download it"
                    ),
            });
            let mut builder = CandidateBuilder::tool_default(
                source,
                context.invocation.intent,
                package_directory.to_path_buf(),
                name,
            )
            .action_key(format!(
                "{framework}:{}:{name}",
                package_scope(manifest, package_directory)
            ))
            .tool(manager.tool())
            .args(manager.local_binary_args(framework_binary_name(framework), &arguments))
            .cwd(package_directory.to_path_buf())
            .env(manager.local_exec_env())
            .selection(selection)
            .base_points(base_points)
            .lifecycle(if context.invocation.intent == Intent::Run {
                Lifecycle::LongRunning
            } else {
                Lifecycle::Finite
            })
            .label(format!("{framework} {name}"))
            .description(&description)
            .evidence(Evidence {
                kind: EvidenceKind::Manifest,
                reason: format!(
                    "package declares {framework} without a canonical `{name}` script"
                ),
                points: 10,
                source: Some(manifest_path.to_path_buf()),
            })
            .evidence(local_binary.as_ref().map_or_else(
                || Evidence {
                    kind: EvidenceKind::Rule,
                    reason: format!(
                        "{} uses its local-only binary execution mode",
                        manager.program()
                    ),
                    points: 0,
                    source: None,
                },
                |path| Evidence {
                    kind: EvidenceKind::Rule,
                    reason: format!("verified project-local {framework} executable"),
                    points: 0,
                    source: Some(path.clone()),
                },
            ))
            .search(SearchDocument {
                identities: vec![name.to_owned(), framework.to_owned()],
                target_paths: vec![manifest_path.to_path_buf()],
                scopes: vec![package_scope(manifest, package_directory)],
                tags: synonyms
                    .iter()
                    .map(|value| (*value).to_owned())
                    .chain(std::iter::once(framework.to_owned()))
                    .collect(),
                text: vec![description, manager.program().to_owned()],
            });
            if let Some(availability) = availability {
                builder = builder.availability(availability);
            }
            builder
                .build()
                .expect("Node framework candidate registration is valid")
        })
        .collect()
}

fn project_local_binary(
    package_directory: &Path,
    scan_root: &Path,
    binary: &str,
) -> Option<PathBuf> {
    package_directory
        .ancestors()
        .take_while(|directory| directory.starts_with(scan_root))
        .find_map(|directory| {
            let bin_directory = directory.join("node_modules").join(".bin");
            ["", ".cmd", ".exe", ".bat", ".com"]
                .into_iter()
                .map(|extension| bin_directory.join(format!("{binary}{extension}")))
                .find(|path| project_binary_file(path))
                .or_else(|| {
                    let pnp = directory.join(".pnp.cjs");
                    pnp.is_file().then_some(pnp)
                })
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
        "sveltekit" => "SvelteKit",
        other => other,
    }
}

fn framework_binary_name(framework: &str) -> &str {
    match framework {
        "sveltekit" => "svelte-kit",
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
        absolute.starts_with(&base.scope_root)
            && closest_node_package(context, &absolute).as_deref()
                == Some(base.scope_root.as_path())
    }

    fn bind(
        &self,
        base: &Candidate,
        target: &IndexEntry,
        context: &ScanCtx<'_>,
    ) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        let relative = absolute.strip_prefix(&base.scope_root).ok()?.to_path_buf();
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
        candidate
            .search
            .target_paths
            .push(target.relative_path.clone());
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

fn bun_native_candidates(
    context: &ScanCtx<'_>,
    manifest: &PackageManifest,
    package_directory: &Path,
    manifest_path: &Path,
) -> Vec<Candidate> {
    let scope = package_scope(manifest, package_directory);
    let synonyms = crate::registry::synonyms(NODE);
    let mut candidates = Vec::new();
    match context.invocation.intent {
        Intent::Run => {
            if !manifest.scripts.contains_key("run") {
                let description = "Bun entry-point auto-detection";
                let candidate = CandidateBuilder::tool_default(
                    NODE_SOURCE,
                    Intent::Run,
                    package_directory.to_path_buf(),
                    "run",
                )
                .action_key(format!("bun:{}:auto-run", scope))
                .tool(BUN_TOOL)
                .args([OsString::from("run")])
                .cwd(package_directory.to_path_buf())
                .selection(SelectionPolicy::ExplicitHint)
                .base_points(40)
                .lifecycle(Lifecycle::LongRunning)
                .label("Bun run")
                .description(description)
                .evidence(Evidence {
                    kind: EvidenceKind::Rule,
                    reason: "Bun is the project package manager; auto-detecting entry point"
                        .to_owned(),
                    points: 0,
                    source: Some(manifest_path.to_path_buf()),
                })
                .search(SearchDocument {
                    identities: vec!["run".to_owned(), "bun".to_owned()],
                    target_paths: vec![manifest_path.to_path_buf()],
                    scopes: vec![scope.clone()],
                    tags: synonyms
                        .iter()
                        .map(|value| (*value).to_owned())
                        .chain(std::iter::once("bun".to_owned()))
                        .collect(),
                    text: vec![description.to_owned()],
                })
                .build()
                .expect("Bun auto-run candidate registration is valid");
                candidates.push(candidate);
            }
        }
        Intent::Build => {
            if !manifest.scripts.contains_key("build") {
                let description = "Bun native bundler";
                let candidate = CandidateBuilder::tool_default(
                    NODE_SOURCE,
                    Intent::Build,
                    package_directory.to_path_buf(),
                    "build",
                )
                .action_key(format!("bun:{}:build", scope))
                .tool(BUN_TOOL)
                .args([OsString::from("build")])
                .cwd(package_directory.to_path_buf())
                .selection(SelectionPolicy::ExplicitHint)
                .base_points(40)
                .label("Bun build")
                .description(description)
                .evidence(Evidence {
                    kind: EvidenceKind::Rule,
                    reason: "Bun is the project package manager; offering native bundler"
                        .to_owned(),
                    points: 0,
                    source: Some(manifest_path.to_path_buf()),
                })
                .search(SearchDocument {
                    identities: vec!["build".to_owned(), "bun".to_owned()],
                    target_paths: vec![manifest_path.to_path_buf()],
                    scopes: vec![scope.clone()],
                    tags: synonyms
                        .iter()
                        .map(|value| (*value).to_owned())
                        .chain(std::iter::once("bun".to_owned()))
                        .collect(),
                    text: vec![description.to_owned()],
                })
                .build()
                .expect("Bun build candidate registration is valid");
                candidates.push(candidate);
            }
        }
        Intent::Test => {
            if has_test_files(context, package_directory) && !manifest.scripts.contains_key("test")
            {
                let description = "Bun native test runner";
                let candidate = CandidateBuilder::tool_default(
                    NODE_SOURCE,
                    Intent::Test,
                    package_directory.to_path_buf(),
                    "test",
                )
                .action_key(format!("bun:{}:test", scope))
                .tool(BUN_TOOL)
                .args([OsString::from("test")])
                .cwd(package_directory.to_path_buf())
                .selection(SelectionPolicy::Automatic)
                .base_points(90)
                .passthrough(PassthroughStyle::DoubleDash)
                .label("Bun test")
                .description(description)
                .evidence(Evidence {
                    kind: EvidenceKind::Rule,
                    reason: "Bun is the project package manager; offering native test runner"
                        .to_owned(),
                    points: 0,
                    source: Some(manifest_path.to_path_buf()),
                })
                .search(SearchDocument {
                    identities: vec!["test".to_owned(), "bun".to_owned()],
                    target_paths: vec![manifest_path.to_path_buf()],
                    scopes: vec![scope.clone()],
                    tags: synonyms
                        .iter()
                        .map(|value| (*value).to_owned())
                        .chain(std::iter::once("bun".to_owned()))
                        .collect(),
                    text: vec![description.to_owned()],
                })
                .build()
                .expect("Bun test candidate registration is valid");
                candidates.push(candidate);
            }
        }
    }
    candidates
}

fn has_test_files(context: &ScanCtx<'_>, package_directory: &Path) -> bool {
    context.index.all_entries().any(|entry| {
        is_javascript_test(&entry.relative_path) && {
            let absolute = context.roots.scan_root.join(&entry.relative_path);
            absolute.starts_with(package_directory)
        }
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn leading_dash_script_names_are_not_commands() {
        assert!(!safe_script_name("--silent"));
        assert!(safe_script_name("dev"));
    }

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
        let env = PackageManager::Pnpm.local_exec_env();
        assert_eq!(
            env.get(&OsString::from("npm_config_verify_deps_before_run")),
            Some(&OsString::from("false"))
        );
    }

    #[test]
    fn workspace_script_arguments_are_manager_specific() {
        let named = WorkspaceMember {
            root: PathBuf::from("/workspace"),
            relative_path: PathBuf::from("apps/web"),
            name: Some("@acme/web".to_owned()),
        };
        let unnamed = WorkspaceMember {
            name: None,
            ..named.clone()
        };
        #[cfg(windows)]
        let unnamed_selector = Path::new(".").join("apps").join("web").into_os_string();
        #[cfg(not(windows))]
        let unnamed_selector = OsString::from("./apps/web");

        let arguments = |manager: PackageManager, member: &WorkspaceMember| {
            manager.script_args("dev", Some(member))
        };

        assert_eq!(
            arguments(PackageManager::Npm, &named),
            ["run", "dev", "--workspace", "@acme/web"].map(OsString::from)
        );
        assert_eq!(
            arguments(PackageManager::Npm, &unnamed),
            [
                OsString::from("run"),
                OsString::from("dev"),
                OsString::from("--workspace"),
                unnamed_selector.clone(),
            ]
        );
        assert_eq!(
            arguments(PackageManager::Pnpm, &named),
            ["--filter", "@acme/web", "run", "dev"].map(OsString::from)
        );
        assert_eq!(
            arguments(PackageManager::Pnpm, &unnamed),
            [
                OsString::from("--filter"),
                unnamed_selector.clone(),
                OsString::from("run"),
                OsString::from("dev"),
            ]
        );
        assert_eq!(
            arguments(PackageManager::Yarn, &named),
            ["workspace", "@acme/web", "run", "dev"].map(OsString::from)
        );
        assert_eq!(
            arguments(PackageManager::Yarn, &unnamed),
            [
                OsString::from("--cwd"),
                unnamed_selector.clone(),
                OsString::from("run"),
                OsString::from("dev"),
            ]
        );
        assert_eq!(
            arguments(PackageManager::Bun, &named),
            ["run", "--filter", "@acme/web", "dev"].map(OsString::from)
        );
        assert_eq!(
            arguments(PackageManager::Bun, &unnamed),
            [
                OsString::from("run"),
                OsString::from("--filter"),
                unnamed_selector,
                OsString::from("dev"),
            ]
        );
    }
}
