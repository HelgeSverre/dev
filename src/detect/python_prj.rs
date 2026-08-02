use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::candidate::{
    Availability, Candidate, Evidence, EvidenceKind, Lifecycle, PassthroughStyle, SearchDocument,
    SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::path::resolve_program;
use crate::registry::{PYTHON3_TOOL, PYTHON_PRJ, PYTHON_PRJ_SOURCE, PYTHON_TOOL, UV_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

pub struct PythonPrjDetector;

#[derive(Clone, Debug, Default, Deserialize)]
struct PyprojectToml {
    project: Option<PyprojectProject>,
    tool: Option<PyprojectTool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PyprojectProject {
    name: Option<String>,
    #[serde(default)]
    scripts: BTreeMap<String, String>,
    #[serde(default, rename = "optional-dependencies")]
    optional_dependencies: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default, Deserialize)]
struct PyprojectTool {
    pytest: Option<PytestConfig>,
    pdm: Option<toml::Value>,
    poetry: Option<toml::Value>,
    uv: Option<toml::Value>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default, Deserialize)]
struct PytestConfig {
    #[serde(default)]
    testpaths: Option<Vec<String>>,
    #[serde(default, rename = "python_files")]
    python_files: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
struct PythonProject {
    manifest_path: PathBuf,
    directory: PathBuf,
    manifest: PyprojectToml,
    managed_by_uv: bool,
    scope: String,
}

impl Detector for PythonPrjDetector {
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

fn projects(context: &ScanCtx<'_>) -> (Vec<PythonProject>, Vec<Diagnostic>) {
    let mut paths = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "pyproject.toml")
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
                diagnostics.push(Diagnostic::warning(
                    PYTHON_PRJ,
                    error.to_string(),
                    Some(absolute),
                ));
                continue;
            }
        };
        let manifest = match toml::from_str::<PyprojectToml>(&contents) {
            Ok(manifest) => manifest,
            Err(error) => {
                diagnostics.push(Diagnostic::warning(
                    PYTHON_PRJ,
                    format!("invalid pyproject.toml: {error}"),
                    Some(absolute),
                ));
                continue;
            }
        };
        let directory = absolute
            .parent()
            .unwrap_or(&context.roots.scan_root)
            .to_path_buf();
        let managed_by_uv = has_uv_lock(context, &directory);
        let scope = project_scope(&manifest, &directory);
        projects.push(PythonProject {
            manifest_path,
            directory,
            manifest,
            managed_by_uv,
            scope,
        });
    }
    (projects, diagnostics)
}

fn project_scope(manifest: &PyprojectToml, directory: &Path) -> String {
    manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .unwrap_or_else(|| {
            directory.file_name().map_or_else(
                || "python-project".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        })
}

fn project_candidates(context: &ScanCtx<'_>, project: &PythonProject) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    let scripts = project.manifest.project.as_ref().map(|proj| &proj.scripts);
    match context.invocation.intent {
        Intent::Run => {
            if let Some(scripts) = scripts {
                for (name, entry) in scripts {
                    if safe_script_name(name) {
                        candidates.push(script_candidate(project, name, entry));
                    }
                }
                if scripts.is_empty() {
                    if let Some(candidate) = default_run_candidate(project) {
                        candidates.push(candidate);
                    }
                }
            } else {
                if let Some(candidate) = default_run_candidate(project) {
                    candidates.push(candidate);
                }
            }
        }
        Intent::Build => {
            if project.managed_by_uv {
                candidates.push(uv_sync_candidate(project));
            }
        }
        Intent::Test => {
            if has_pytest(project) {
                candidates.push(pytest_candidate(project));
            }
        }
    }
    candidates
}

fn safe_script_name(name: &str) -> bool {
    !name.starts_with('-') && !name.is_empty()
}

fn script_candidate(project: &PythonProject, name: &str, entry: &str) -> Candidate {
    let description = format!("Declared project script entry point `{name}`");
    let (args, tool) = if project.managed_by_uv {
        let args = vec![
            OsString::from("run"),
            OsString::from("--no-sync"),
            OsString::from(name),
        ];
        (args, Some(UV_TOOL))
    } else {
        let args: Vec<OsString> = entry.split_whitespace().map(OsString::from).collect();
        (args, Some(python_interpreter_tool(&project.directory)))
    };
    let mut builder = CandidateBuilder::ecosystem_task(
        PYTHON_PRJ_SOURCE,
        Intent::Run,
        project.directory.clone(),
        name,
    )
    .action_key(format!("python-prj:{}:script:{}", project.scope, name))
    .args(args)
    .cwd(project.directory.clone())
    .selection(SelectionPolicy::ExplicitHint)
    .base_points(35)
    .lifecycle(Lifecycle::LongRunning)
    .passthrough(PassthroughStyle::DoubleDash)
    .label(format!("Python script `{name}`"))
    .description(&description)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: format!("pyproject.toml declares script `{name}`"),
        points: 0,
        source: Some(project.manifest_path.clone()),
    })
    .search(SearchDocument {
        identities: vec![name.to_owned()],
        target_paths: vec![project.manifest_path.clone()],
        scopes: vec![project.scope.clone()],
        tags: vec!["python".to_owned(), "py".to_owned()],
        text: vec![description],
    });
    if let Some(tool) = tool {
        builder = builder.tool(tool);
    }
    builder
        .build()
        .expect("Python script candidate registration is valid")
}

fn default_run_candidate(project: &PythonProject) -> Option<Candidate> {
    let (args, tool) = if project.managed_by_uv {
        (
            vec![OsString::from("run"), OsString::from("--no-sync")],
            UV_TOOL,
        )
    } else {
        return None;
    };
    let description = "Runs the project entry point via uv";
    Some(
        CandidateBuilder::tool_default(
            PYTHON_PRJ_SOURCE,
            Intent::Run,
            project.directory.clone(),
            "run",
        )
        .action_key(format!("python-prj:{}:run", project.scope))
        .tool(tool)
        .args(args)
        .cwd(project.directory.clone())
        .selection(SelectionPolicy::Automatic)
        .base_points(95)
        .lifecycle(Lifecycle::LongRunning)
        .passthrough(PassthroughStyle::DoubleDash)
        .label("Python project run")
        .description(description)
        .evidence(Evidence {
            kind: EvidenceKind::Manifest,
            reason: "pyproject.toml declares a Python project managed by uv".to_owned(),
            points: 0,
            source: Some(project.manifest_path.clone()),
        })
        .search(SearchDocument {
            identities: vec!["run".to_owned(), "python".to_owned()],
            target_paths: vec![project.manifest_path.clone()],
            scopes: vec![project.scope.clone()],
            tags: vec!["python".to_owned(), "py".to_owned()],
            text: vec![description.to_owned()],
        })
        .build()
        .expect("Python default run candidate registration is valid"),
    )
}

fn uv_sync_candidate(project: &PythonProject) -> Candidate {
    let description = "Synchronize the project environment via uv sync";
    CandidateBuilder::tool_default(
        PYTHON_PRJ_SOURCE,
        Intent::Build,
        project.directory.clone(),
        "sync",
    )
    .action_key(format!("python-prj:{}:sync", project.scope))
    .tool(UV_TOOL)
    .args([OsString::from("sync")])
    .cwd(project.directory.clone())
    .selection(SelectionPolicy::ExplicitHint)
    .base_points(60)
    .label("uv sync")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: "pyproject.toml declares a Python project managed by uv".to_owned(),
        points: 0,
        source: Some(project.manifest_path.clone()),
    })
    .search(SearchDocument {
        identities: vec!["sync".to_owned(), "build".to_owned(), "install".to_owned()],
        target_paths: vec![project.manifest_path.clone()],
        scopes: vec![project.scope.clone()],
        tags: vec!["python".to_owned(), "py".to_owned(), "uv".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("uv sync candidate registration is valid")
}

fn has_pytest(project: &PythonProject) -> bool {
    if project.manifest.project.as_ref().is_some_and(|proj| {
        proj.dependencies.iter().any(|d| d.starts_with("pytest"))
            || proj
                .optional_dependencies
                .iter()
                .any(|(_, deps)| deps.iter().any(|d| d.starts_with("pytest")))
    }) {
        return true;
    }
    project
        .manifest
        .tool
        .as_ref()
        .and_then(|tool| tool.pytest.as_ref())
        .is_some()
}

fn pytest_candidate(project: &PythonProject) -> Candidate {
    let (args, tool) = if project.managed_by_uv {
        (
            vec![
                OsString::from("run"),
                OsString::from("--no-sync"),
                OsString::from("pytest"),
            ],
            UV_TOOL,
        )
    } else {
        let tool = python_interpreter_tool(&project.directory);
        let args = vec![OsString::from("-m"), OsString::from("pytest")];
        (args, tool)
    };
    let description = "Run project test suite via pytest";
    CandidateBuilder::tool_default(
        PYTHON_PRJ_SOURCE,
        Intent::Test,
        project.directory.clone(),
        "test",
    )
    .action_key(format!("python-prj:{}:test", project.scope))
    .tool(tool)
    .args(args)
    .cwd(project.directory.clone())
    .selection(SelectionPolicy::Automatic)
    .base_points(90)
    .passthrough(PassthroughStyle::DoubleDash)
    .label("pytest")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: "pyproject.toml declares pytest as a project dependency".to_owned(),
        points: 0,
        source: Some(project.manifest_path.clone()),
    })
    .search(SearchDocument {
        identities: vec!["test".to_owned(), "pytest".to_owned()],
        target_paths: vec![project.manifest_path.clone()],
        scopes: vec![project.scope.clone()],
        tags: vec!["python".to_owned(), "py".to_owned(), "pytest".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("pytest candidate registration is valid")
}

fn has_uv_lock(context: &ScanCtx<'_>, directory: &Path) -> bool {
    let relative = match directory.strip_prefix(&context.roots.scan_root) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => return false,
    };
    context.index.all_entries().any(|entry| {
        entry.file_type == IndexedFileType::File
            && entry
                .relative_path
                .file_name()
                .is_some_and(|name| name == "uv.lock")
            && {
                let parent = entry.relative_path.parent();
                parent == Some(Path::new("")) || parent == Some(&relative)
            }
    })
}

fn python_interpreter_tool(cwd: &Path) -> crate::registry::ToolId {
    if let Some(virtual_environment) = std::env::var_os("VIRTUAL_ENV") {
        let root = PathBuf::from(virtual_environment);
        #[cfg(windows)]
        let interpreter = root.join("Scripts/python.exe");
        #[cfg(not(windows))]
        let interpreter = root.join("bin/python");
        if matches!(
            resolve_program(interpreter.as_os_str(), cwd, &BTreeMap::new()),
            Availability::Available { .. }
        ) {
            return PYTHON_TOOL;
        }
    }
    if matches!(
        resolve_program(OsStr::new("python3"), cwd, &BTreeMap::new()),
        Availability::Available { .. }
    ) {
        PYTHON3_TOOL
    } else {
        PYTHON_TOOL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_pyproject_toml() {
        let toml = r#"[project]
name = "gemma"
version = "0.1.0"
dependencies = ["requests", "python-dotenv"]
"#;
        let manifest = toml::from_str::<PyprojectToml>(toml).unwrap();
        let project = manifest.project.unwrap();
        assert_eq!(project.name.as_deref(), Some("gemma"));
        assert_eq!(project.dependencies.len(), 2);
        assert!(project.scripts.is_empty());
    }

    #[test]
    fn parses_pyproject_with_scripts() {
        let toml = r#"[project]
name = "myapp"
dependencies = ["pytest"]

[project.scripts]
serve = "myapp.main:serve"
build = "myapp.main:build"
"#;
        let manifest = toml::from_str::<PyprojectToml>(toml).unwrap();
        let project = manifest.project.unwrap();
        assert_eq!(project.scripts.len(), 2);
        assert!(project.scripts.contains_key("serve"));
    }

    #[test]
    fn no_scripts_is_none_not_empty_map() {
        let toml = r#"[project]
name = "minimal"
"#;
        let manifest = toml::from_str::<PyprojectToml>(toml).unwrap();
        let project = manifest.project.unwrap();
        assert!(project.scripts.is_empty());
    }
}
