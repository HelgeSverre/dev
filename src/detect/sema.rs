use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::candidate::{Evidence, EvidenceKind, PassthroughStyle, SearchDocument, SelectionPolicy};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{SEMA, SEMA_SOURCE, SEMA_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

pub struct SemaDetector;

#[derive(Clone, Debug)]
struct SemaProject {
    manifest: PathBuf,
    directory: PathBuf,
    name: Option<String>,
    description: Option<String>,
    entrypoint: Option<PathBuf>,
    package: bool,
}

impl Detector for SemaDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let (projects, mut diagnostics) = projects(context);
        let mut output = Detection {
            candidates: Vec::new(),
            diagnostics: Vec::new(),
        };
        output.diagnostics.append(&mut diagnostics);
        let mut declared_entrypoints = BTreeSet::new();
        for project in &projects {
            if let Some(entrypoint) = &project.entrypoint {
                declared_entrypoints.insert(project.directory.join(entrypoint));
                emit_project_entrypoint(context, project, entrypoint, &mut output);
            }
            if context.invocation.intent == Intent::Test && project.package {
                emit_project_test(context, project, &mut output);
            }
        }
        emit_direct_files(context, &declared_entrypoints, &mut output);
        output
    }
}

fn projects(context: &ScanCtx<'_>) -> (Vec<SemaProject>, Vec<Diagnostic>) {
    let mut manifests = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "sema.toml")
        })
        .map(|entry| context.roots.scan_root.join(&entry.relative_path))
        .collect::<Vec<_>>();
    manifests.sort();
    manifests.dedup();

    let mut projects = Vec::new();
    let mut diagnostics = Vec::new();
    for manifest in manifests {
        let contents = match context.index.manifests.read(&manifest) {
            Ok(contents) => contents,
            Err(error) => {
                diagnostics.push(Diagnostic::warning(SEMA, error.to_string(), Some(manifest)));
                continue;
            }
        };
        let document = match toml::from_str::<toml::Value>(&contents) {
            Ok(document) => document,
            Err(error) => {
                diagnostics.push(Diagnostic::warning(
                    SEMA,
                    format!("invalid sema.toml: {error}"),
                    Some(manifest),
                ));
                continue;
            }
        };
        let package = document.get("package").and_then(toml::Value::as_table);
        let entrypoint = package
            .and_then(|package| package.get("entrypoint"))
            .and_then(toml::Value::as_str)
            .or_else(|| document.get("entrypoint").and_then(toml::Value::as_str))
            .map(PathBuf::from)
            .or_else(|| package.is_some().then(|| PathBuf::from("package.sema")));
        let entrypoint = entrypoint.filter(|path| safe_relative_path(path));
        let directory = manifest
            .parent()
            .unwrap_or(&context.roots.scan_root)
            .to_path_buf();
        let entrypoint = entrypoint.filter(|entrypoint| directory.join(entrypoint).is_file());
        projects.push(SemaProject {
            manifest,
            directory,
            name: package
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
                .map(str::to_owned),
            description: package
                .and_then(|package| package.get("description"))
                .and_then(toml::Value::as_str)
                .map(str::to_owned),
            entrypoint,
            package: package.is_some(),
        });
    }
    (projects, diagnostics)
}

fn emit_project_entrypoint(
    context: &ScanCtx<'_>,
    project: &SemaProject,
    entrypoint: &Path,
    output: &mut Detection,
) {
    let (action, mut args, points) = match context.invocation.intent {
        Intent::Run => ("run", Vec::new(), 90),
        Intent::Build => ("build", vec![OsString::from("build")], 90),
        Intent::Test => return,
    };
    args.push(entrypoint.as_os_str().to_owned());
    let relative_manifest = project
        .manifest
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(&project.manifest);
    let name = project.name.as_deref().unwrap_or("sema-package");
    let description = project
        .description
        .clone()
        .unwrap_or_else(|| format!("Sema package entrypoint {}", entrypoint.display()));
    CandidateBuilder::tool_default(
        SEMA_SOURCE,
        context.invocation.intent,
        project.directory.clone(),
        action,
    )
    .action_key(format!(
        "sema:{}:{action}",
        relative_manifest
            .to_string_lossy()
            .replace(['/', '\\'], ":")
    ))
    .tool(SEMA_TOOL)
    .args(args)
    .cwd(project.directory.clone())
    .passthrough(if context.invocation.intent == Intent::Run {
        PassthroughStyle::DoubleDash
    } else {
        PassthroughStyle::Append
    })
    .selection(SelectionPolicy::Automatic)
    .base_points(points)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: format!(
            "{} declares Sema entrypoint `{}`",
            relative_manifest.display(),
            entrypoint.display()
        ),
        points: 0,
        source: Some(relative_manifest.to_path_buf()),
    })
    .search(SearchDocument {
        identities: vec![action.to_owned(), name.to_owned()],
        target_paths: vec![relative_manifest.to_path_buf(), entrypoint.to_path_buf()],
        scopes: vec![name.to_owned()],
        tags: Vec::new(),
        text: vec![description.clone()],
    })
    .label(format!("Sema {action} `{name}`"))
    .description(description)
    .emit(output);
}

fn emit_project_test(context: &ScanCtx<'_>, project: &SemaProject, output: &mut Detection) {
    let relative_manifest = project
        .manifest
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(&project.manifest);
    let name = project.name.as_deref().unwrap_or("sema-package");
    CandidateBuilder::tool_default(SEMA_SOURCE, Intent::Test, project.directory.clone(), "test")
        .action_key(format!(
            "sema:{}:test",
            relative_manifest
                .to_string_lossy()
                .replace(['/', '\\'], ":")
        ))
        .tool(SEMA_TOOL)
        .args(["test"])
        .cwd(project.directory.clone())
        .passthrough(PassthroughStyle::Append)
        .selection(SelectionPolicy::Automatic)
        .base_points(90)
        .evidence(Evidence {
            kind: EvidenceKind::Manifest,
            reason: format!(
                "{} declares Sema package `{name}`",
                relative_manifest.display()
            ),
            points: 0,
            source: Some(relative_manifest.to_path_buf()),
        })
        .search(SearchDocument {
            identities: vec!["test".to_owned(), name.to_owned()],
            target_paths: vec![relative_manifest.to_path_buf()],
            scopes: vec![name.to_owned()],
            tags: Vec::new(),
            text: vec!["Run ordinary Sema tests".to_owned()],
        })
        .label(format!("Sema tests `{name}`"))
        .description("Run ordinary Sema tests")
        .emit(output);
}

fn emit_direct_files(
    context: &ScanCtx<'_>,
    declared_entrypoints: &BTreeSet<PathBuf>,
    output: &mut Detection,
) {
    for entry in context.index.all_entries() {
        if entry.file_type != IndexedFileType::File
            || entry
                .relative_path
                .extension()
                .is_none_or(|extension| extension != "sema")
        {
            continue;
        }
        let absolute = context.roots.scan_root.join(&entry.relative_path);
        if declared_entrypoints.contains(&absolute) {
            continue;
        }
        let is_test = entry
            .relative_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".test.sema"));
        if context.invocation.intent == Intent::Test && !is_test {
            continue;
        }
        let directory = absolute
            .parent()
            .unwrap_or(&context.roots.scan_root)
            .to_path_buf();
        let name = entry.relative_path.file_stem().map_or_else(
            || "sema".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let (action, args) = match context.invocation.intent {
            Intent::Run => ("run", vec![absolute.as_os_str().to_owned()]),
            Intent::Build => (
                "build",
                vec![OsString::from("build"), absolute.as_os_str().to_owned()],
            ),
            Intent::Test => (
                "test",
                vec![OsString::from("test"), absolute.as_os_str().to_owned()],
            ),
        };
        CandidateBuilder::direct_target(
            SEMA_SOURCE,
            context.invocation.intent,
            directory.clone(),
            &name,
        )
        .action_key(format!(
            "sema:file:{}:{action}",
            entry
                .relative_path
                .to_string_lossy()
                .replace(['/', '\\'], ":")
        ))
        .tool(SEMA_TOOL)
        .args(args)
        .cwd(directory)
        .passthrough(if context.invocation.intent == Intent::Run {
            PassthroughStyle::DoubleDash
        } else {
            PassthroughStyle::Append
        })
        .selection(SelectionPolicy::ExplicitHint)
        .base_points(25)
        .evidence(Evidence {
            kind: EvidenceKind::Rule,
            reason: format!("{} is a Sema source file", entry.relative_path.display()),
            points: 0,
            source: Some(entry.relative_path.clone()),
        })
        .search(SearchDocument {
            identities: vec![name.clone(), action.to_owned()],
            target_paths: vec![entry.relative_path.clone()],
            scopes: Vec::new(),
            tags: Vec::new(),
            text: vec![format!(
                "Sema source file {}",
                entry.relative_path.display()
            )],
        })
        .label(format!("Sema file {}", entry.relative_path.display()))
        .description(format!("Sema source file for `{action}`"))
        .emit(output);
    }
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entrypoints_must_stay_inside_the_package() {
        assert!(safe_relative_path(Path::new("package.sema")));
        assert!(safe_relative_path(Path::new("src/main.sema")));
        assert!(!safe_relative_path(Path::new("../main.sema")));
        assert!(!safe_relative_path(Path::new("/tmp/main.sema")));
    }
}
