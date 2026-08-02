use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::candidate::{
    Availability, Candidate, CandidateOrigin, CommandLayer, Evidence, EvidenceKind, Lifecycle,
    SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::{Intent, Target};
use crate::query::{match_candidate, normalize_query, MatchClass};
use crate::registry::{DART, DART_SOURCE, DART_TOOL, FLUTTER_SOURCE, FLUTTER_TOOL};
use crate::scan::{IndexEntry, IndexedFileType};

use super::target::{explicitly_anchored, target_scope};
use super::{CandidateBuilder, Detection, Detector, ScanCtx, TargetRunner};

pub struct DartDetector;

#[derive(Clone, Debug, Default, Deserialize)]
struct Pubspec {
    name: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, serde_yaml::Value>,
    #[serde(default, rename = "dev_dependencies")]
    dev_dependencies: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Clone, Debug)]
struct DartProject {
    manifest_path: PathBuf,
    directory: PathBuf,
    manifest: Pubspec,
    flutter: bool,
}

impl Detector for DartDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let (projects, diagnostics) = projects(context);
        let mut output = Detection {
            candidates: Vec::new(),
            diagnostics,
        };
        for project in &projects {
            output
                .candidates
                .extend(project_candidates(context, project));
        }
        output
    }
}

impl TargetRunner for DartDetector {
    fn supports(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
        context.invocation.intent == Intent::Run && is_dart_file(target)
    }

    fn candidate(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        let explicit = explicitly_anchored(target, context);
        let (projects, _) = projects(context);
        let mut candidate = if let Some(project) = closest_project(&absolute, &projects) {
            let relative = absolute.strip_prefix(&project.directory).ok()?;
            dart_run_candidate(project, Some(relative), explicit)
        } else {
            standalone_without_pubspec(&absolute, explicit)
        };
        candidate.origin = if explicit {
            CandidateOrigin::Declared
        } else {
            CandidateOrigin::Synthetic
        };
        candidate.layer = CommandLayer::DirectTarget;
        Some(candidate)
    }
}

fn projects(context: &ScanCtx<'_>) -> (Vec<DartProject>, Vec<Diagnostic>) {
    let mut paths = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "pubspec.yaml")
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
                diagnostics.push(Diagnostic::warning(DART, error.to_string(), Some(absolute)));
                continue;
            }
        };
        let manifest = match serde_yaml::from_str::<Pubspec>(&contents) {
            Ok(manifest) => manifest,
            Err(error) => {
                diagnostics.push(Diagnostic::warning(
                    DART,
                    format!("invalid pubspec.yaml: {error}"),
                    Some(absolute),
                ));
                continue;
            }
        };
        let flutter = manifest
            .dependencies
            .get("flutter")
            .or_else(|| manifest.dev_dependencies.get("flutter"))
            .is_some_and(is_flutter_sdk_dependency);
        projects.push(DartProject {
            directory: absolute
                .parent()
                .unwrap_or(&context.roots.scan_root)
                .to_path_buf(),
            manifest_path,
            manifest,
            flutter,
        });
    }
    (projects, diagnostics)
}

fn project_candidates(context: &ScanCtx<'_>, project: &DartProject) -> Vec<Candidate> {
    if project.flutter {
        return flutter_candidates(context, project);
    }
    match context.invocation.intent {
        Intent::Run => default_dart_entry(project)
            .map(|entry| vec![dart_run_candidate(project, entry.as_deref(), true)])
            .unwrap_or_default(),
        Intent::Build => Vec::new(),
        Intent::Test => dart_test_candidates(context, project),
    }
}

fn flutter_candidates(context: &ScanCtx<'_>, project: &DartProject) -> Vec<Candidate> {
    match context.invocation.intent {
        Intent::Run => {
            let mut candidate = base_project_candidate(
                project,
                "flutter",
                Intent::Run,
                "run",
                vec![OsString::from("run")],
                95,
                SelectionPolicy::Automatic,
            );
            candidate.lifecycle = Lifecycle::LongRunning;
            candidate.label = "Flutter application".to_owned();
            candidate.description =
                "Runs Flutter; the SDK may prompt for a connected device or target".to_owned();
            vec![candidate]
        }
        Intent::Test => flutter_test_candidates(context, project),
        Intent::Build => flutter_build_candidates(project),
    }
}

fn flutter_build_candidates(project: &DartProject) -> Vec<Candidate> {
    [
        ("android", "appbundle"),
        ("ios", "ipa"),
        ("web", "web"),
        ("macos", "macos"),
        ("windows", "windows"),
        ("linux", "linux"),
    ]
    .into_iter()
    .filter(|(directory, _)| project.directory.join(directory).is_dir())
    .map(|(platform, build_target)| {
        let mut candidate = base_project_candidate(
            project,
            "flutter",
            Intent::Build,
            platform,
            vec![OsString::from("build"), OsString::from(build_target)],
            50,
            SelectionPolicy::ExplicitHint,
        );
        candidate.label = format!("Flutter {platform} build");
        candidate.description = format!("Builds the generated {platform} platform target");
        candidate.search.identities.push(platform.to_owned());
        candidate.search.target_paths.push(PathBuf::from(platform));
        if let Some(reason) = unsupported_flutter_host(platform) {
            candidate.availability = Availability::UnsupportedHost { reason };
        }
        candidate
    })
    .collect()
}

fn dart_run_candidate(project: &DartProject, target: Option<&Path>, automatic: bool) -> Candidate {
    let mut args = vec![OsString::from("run")];
    args.extend(target.iter().map(|path| path.as_os_str().to_owned()));
    let identity = target.and_then(Path::file_stem).map_or_else(
        || project_scope(project),
        |name| name.to_string_lossy().into_owned(),
    );
    let mut candidate = base_project_candidate(
        project,
        "dart",
        Intent::Run,
        "run",
        args,
        if automatic { 95 } else { 30 },
        if automatic {
            SelectionPolicy::Automatic
        } else {
            SelectionPolicy::ExplicitHint
        },
    );
    candidate.label = target.map_or_else(
        || format!("Dart package {identity}"),
        |path| format!("Dart file {}", path.display()),
    );
    candidate.description = "Runs a Dart package or source target".to_owned();
    if let Some(target) = target {
        candidate.search.identities.push(identity);
        candidate.search.target_paths.push(target.to_path_buf());
        candidate.action_key = format!(
            "{}:target:{}",
            candidate.action_key,
            normalized_target_suffix(target)
        );
    }
    candidate
}

fn dart_test_candidates(context: &ScanCtx<'_>, project: &DartProject) -> Vec<Candidate> {
    test_targets(context, project)
        .into_iter()
        .map(|target| dart_test_candidate(project, target.as_deref()))
        .collect()
}

fn dart_test_candidate(project: &DartProject, target: Option<&Path>) -> Candidate {
    let mut args = vec![OsString::from("test")];
    args.extend(target.iter().map(|path| path.as_os_str().to_owned()));
    let mut candidate = base_project_candidate(
        project,
        "dart",
        Intent::Test,
        "test",
        args,
        95,
        SelectionPolicy::Automatic,
    );
    candidate.label = target.map_or_else(
        || "Dart tests".to_owned(),
        |path| format!("Dart test {}", path.display()),
    );
    bind_test_target(&mut candidate, target);
    candidate
}

fn flutter_test_candidates(context: &ScanCtx<'_>, project: &DartProject) -> Vec<Candidate> {
    test_targets(context, project)
        .into_iter()
        .map(|target| flutter_test_candidate(project, target.as_deref()))
        .collect()
}

fn flutter_test_candidate(project: &DartProject, target: Option<&Path>) -> Candidate {
    let mut args = vec![OsString::from("test")];
    args.extend(target.iter().map(|path| path.as_os_str().to_owned()));
    let mut candidate = base_project_candidate(
        project,
        "flutter",
        Intent::Test,
        "test",
        args,
        95,
        SelectionPolicy::Automatic,
    );
    candidate.label = target.map_or_else(
        || "Flutter tests".to_owned(),
        |path| format!("Flutter test {}", path.display()),
    );
    bind_test_target(&mut candidate, target);
    candidate
}

fn base_project_candidate(
    project: &DartProject,
    source_name: &'static str,
    intent: Intent,
    action: &str,
    args: Vec<OsString>,
    base_points: i32,
    selection: SelectionPolicy,
) -> Candidate {
    let scope = project_scope(project);
    let source = if source_name == "flutter" {
        FLUTTER_SOURCE
    } else {
        DART_SOURCE
    };
    let tool = if source_name == "flutter" {
        FLUTTER_TOOL
    } else {
        DART_TOOL
    };
    let description = format!("{source_name} project command");
    CandidateBuilder::tool_default(source, intent, project.directory.clone(), action)
        .action_key(format!("{source_name}:{scope}:{action}"))
        .tool(tool)
        .args(args)
        .cwd(project.directory.clone())
        .selection(selection)
        .base_points(base_points)
        .label(format!("{source_name} {action}"))
        .description(&description)
        .evidence(Evidence {
            kind: EvidenceKind::Manifest,
            reason: format!(
                "pubspec.yaml declares a {} project",
                if project.flutter { "Flutter" } else { "Dart" }
            ),
            points: 0,
            source: Some(project.manifest_path.clone()),
        })
        .search(SearchDocument {
            identities: vec![action.to_owned()],
            target_paths: vec![project.manifest_path.clone()],
            scopes: vec![scope],
            tags: vec![
                "dart".to_owned(),
                if project.flutter {
                    "flutter".to_owned()
                } else {
                    "pub".to_owned()
                },
            ],
            text: vec![description],
        })
        .build()
        .expect("Dart project candidate registration is valid")
}

fn bind_test_target(candidate: &mut Candidate, target: Option<&Path>) {
    if let Some(target) = target {
        candidate.search.identities.extend(
            target
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned()),
        );
        candidate.search.target_paths.push(target.to_path_buf());
        candidate.evidence.push(Evidence {
            kind: EvidenceKind::Rule,
            reason: format!("bound test provider to {}", target.display()),
            points: 20,
            source: Some(target.to_path_buf()),
        });
        candidate.action_key = format!(
            "{}:target:{}",
            candidate.action_key,
            normalized_target_suffix(target)
        );
    }
}

fn test_targets(context: &ScanCtx<'_>, project: &DartProject) -> Vec<Option<PathBuf>> {
    if let Some(target) = explicit_dart_test_target(context, project) {
        return vec![Some(target)];
    }
    let mut targets = vec![None];
    if context.invocation.hints.is_empty() {
        return targets;
    }
    let query = normalize_query(&context.invocation.hints);
    let test_directory = project.directory.join("test");
    targets.extend(
        context
            .index
            .all_entries()
            .filter(|entry| is_dart_file(entry))
            .filter_map(|entry| {
                let absolute = context.roots.scan_root.join(&entry.relative_path);
                if !absolute.starts_with(&test_directory) {
                    return None;
                }
                let relative = absolute
                    .strip_prefix(&project.directory)
                    .ok()?
                    .to_path_buf();
                let candidate = if project.flutter {
                    flutter_test_candidate(project, Some(&relative))
                } else {
                    dart_test_candidate(project, Some(&relative))
                };
                let matched = match_candidate(&candidate, &query, context.invocation.chaos);
                (matched.highest_class == Some(MatchClass::Identity)
                    && matched.matched_meaningful_terms > 0)
                    .then_some(Some(relative))
            }),
    );
    targets
}

fn explicit_dart_test_target(context: &ScanCtx<'_>, project: &DartProject) -> Option<PathBuf> {
    let Target::File(target) = &context.invocation.target else {
        return None;
    };
    (target
        .extension()
        .is_some_and(|extension| extension == "dart")
        && target.starts_with(project.directory.join("test")))
    .then(|| {
        target
            .strip_prefix(&project.directory)
            .unwrap_or(target)
            .to_path_buf()
    })
}

fn default_dart_entry(project: &DartProject) -> Option<Option<PathBuf>> {
    let bin = project.directory.join("bin");
    let package_name = project.manifest.name.as_deref()?;
    let conventional = bin.join(format!("{package_name}.dart"));
    if conventional.is_file() {
        return Some(None);
    }
    let mut entries = std::fs::read_dir(bin)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "dart")
        })
        .take(2)
        .collect::<Vec<_>>();
    if entries.len() == 1 {
        let relative = entries
            .pop()?
            .strip_prefix(&project.directory)
            .ok()?
            .to_path_buf();
        Some(Some(relative))
    } else {
        None
    }
}

fn standalone_without_pubspec(target: &Path, explicit: bool) -> Candidate {
    let directory = target.parent().unwrap_or(Path::new(".")).to_path_buf();
    let filename = target
        .file_name()
        .map_or_else(|| OsString::from("main.dart"), std::ffi::OsStr::to_owned);
    let identity = target.file_stem().map_or_else(
        || "main".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let description = "Standalone Dart source target".to_owned();
    CandidateBuilder::direct_target(DART_SOURCE, Intent::Run, directory.clone(), &identity)
        .action_key(format!("dart:file:{}", normalized_target_suffix(target)))
        .tool(DART_TOOL)
        .args([OsString::from("run"), filename.clone()])
        .cwd(directory)
        .selection(if explicit {
            SelectionPolicy::Automatic
        } else {
            SelectionPolicy::ExplicitHint
        })
        .base_points(if explicit { 95 } else { 25 })
        .origin(if explicit {
            CandidateOrigin::Declared
        } else {
            CandidateOrigin::Synthetic
        })
        .label(format!("Dart file {}", target.display()))
        .description(&description)
        .evidence(Evidence {
            kind: EvidenceKind::Rule,
            reason: "selected dart run for a standalone .dart target".to_owned(),
            points: 0,
            source: Some(target.to_path_buf()),
        })
        .search(SearchDocument {
            identities: vec![identity, filename.to_string_lossy().into_owned()],
            target_paths: vec![PathBuf::from(filename), target.to_path_buf()],
            scopes: vec![target_scope(target)],
            tags: vec!["dart".to_owned()],
            text: vec![description],
        })
        .build()
        .expect("Dart file candidate registration is valid")
}

fn normalized_target_suffix(target: &Path) -> String {
    target.to_string_lossy().replace(['/', '\\'], ":")
}

fn closest_project<'a>(target: &Path, projects: &'a [DartProject]) -> Option<&'a DartProject> {
    projects
        .iter()
        .filter(|project| target.starts_with(&project.directory))
        .max_by_key(|project| project.directory.components().count())
}

fn project_scope(project: &DartProject) -> String {
    project.manifest.name.clone().unwrap_or_else(|| {
        project.directory.file_name().map_or_else(
            || "dart-project".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    })
}

fn is_flutter_sdk_dependency(value: &serde_yaml::Value) -> bool {
    value
        .as_mapping()
        .and_then(|mapping| mapping.get(serde_yaml::Value::String("sdk".to_owned())))
        .and_then(serde_yaml::Value::as_str)
        == Some("flutter")
}

fn is_dart_file(entry: &IndexEntry) -> bool {
    entry.file_type != IndexedFileType::Directory
        && entry
            .relative_path
            .extension()
            .is_some_and(|extension| extension == "dart")
}

fn unsupported_flutter_host(platform: &str) -> Option<String> {
    let supported = match platform {
        "ios" | "macos" => cfg!(target_os = "macos"),
        "windows" => cfg!(target_os = "windows"),
        "linux" => cfg!(target_os = "linux"),
        "android" | "web" => true,
        _ => false,
    };
    (!supported).then(|| format!("Flutter {platform} builds are unsupported on this host"))
}
