use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use serde_yaml::{Mapping, Value};

use crate::candidate::{Evidence, EvidenceKind, PassthroughStyle, SearchDocument, SelectionPolicy};
use crate::diagnostic::Diagnostic;
use crate::registry::{TASKFILE, TASKFILE_SOURCE, TASK_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_INCLUDED_FILES: usize = 64;
const TASKFILE_NAMES: &[&str] = &[
    "Taskfile.yml",
    "Taskfile.yaml",
    "taskfile.yml",
    "taskfile.yaml",
    "Taskfile.dist.yml",
    "Taskfile.dist.yaml",
    "taskfile.dist.yml",
    "taskfile.dist.yaml",
];

pub struct TaskfileDetector;

#[derive(Clone, Debug)]
struct TaskDefinition {
    name: String,
    invocation: String,
    source: PathBuf,
    description: Option<String>,
    aliases: Vec<String>,
    requires_confirmation: bool,
}

struct ParseState<'a> {
    context: &'a ScanCtx<'a>,
    tasks: BTreeMap<String, TaskDefinition>,
    visited: BTreeSet<(PathBuf, String)>,
    files: BTreeSet<PathBuf>,
    diagnostics: Vec<Diagnostic>,
    invalid: bool,
}

impl Detector for TaskfileDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut output = Detection::default();
        for (root_file, directory) in taskfiles(context) {
            detect_file(context, &root_file, &directory, &mut output);
        }
        output
    }
}

fn is_taskfile(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| TASKFILE_NAMES.contains(&name))
}

fn taskfiles(context: &ScanCtx<'_>) -> Vec<(PathBuf, PathBuf)> {
    let mut by_directory = BTreeMap::<PathBuf, (usize, PathBuf)>::new();
    for entry in context.index.all_entries() {
        if entry.file_type != IndexedFileType::File || !is_taskfile(&entry.relative_path) {
            continue;
        }
        let Some(name) = entry
            .relative_path
            .file_name()
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        let priority = TASKFILE_NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .unwrap_or(usize::MAX);
        let absolute = context.roots.scan_root.join(&entry.relative_path);
        let directory = absolute
            .parent()
            .unwrap_or(&context.roots.scan_root)
            .to_path_buf();
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
        .into_iter()
        .map(|(directory, (_, file))| (file, directory))
        .collect()
}

fn detect_file(context: &ScanCtx<'_>, root_file: &Path, directory: &Path, output: &mut Detection) {
    let mut state = ParseState {
        context,
        tasks: BTreeMap::new(),
        visited: BTreeSet::new(),
        files: BTreeSet::new(),
        diagnostics: Vec::new(),
        invalid: false,
    };
    parse_file(root_file, "", 0, &mut state);
    output.diagnostics.append(&mut state.diagnostics);
    if state.invalid {
        return;
    }

    let relative_root = root_file
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(root_file);
    let scope = directory.file_name().map_or_else(
        || "task-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    for task in state.tasks.into_values() {
        let Some((mut selection, points)) = super::task_facade::policy(
            context.invocation.intent,
            task.aliases
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(task.name.as_str())),
            task.name == "default",
        ) else {
            continue;
        };
        if task.requires_confirmation && selection == SelectionPolicy::Automatic {
            selection = SelectionPolicy::Confirm;
        }
        let description = task
            .description
            .clone()
            .unwrap_or_else(|| format!("Public task from {}", relative_root.display()));
        let mut identities = vec![task.name.clone(), task.invocation.clone()];
        identities.extend(task.aliases.clone());
        identities.sort();
        identities.dedup();
        CandidateBuilder::project_facade(
            TASKFILE_SOURCE,
            context.invocation.intent,
            directory.to_path_buf(),
            &task.name,
        )
        .action_key(format!(
            "taskfile:{}:{}",
            relative_root.to_string_lossy().replace(['/', '\\'], ":"),
            task.invocation
        ))
        .tool(TASK_TOOL)
        .args([
            OsString::from("--taskfile"),
            root_file.as_os_str().to_owned(),
            OsString::from(&task.invocation),
        ])
        .cwd(directory.to_path_buf())
        .passthrough(PassthroughStyle::DoubleDash)
        .selection(selection)
        .base_points(points)
        .lifecycle(super::task_facade::lifecycle(
            context.invocation.intent,
            task.aliases
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(task.name.as_str())),
        ))
        .evidence(Evidence {
            kind: EvidenceKind::Manifest,
            reason: format!(
                "{} declares task `{}`",
                task.source.display(),
                task.invocation
            ),
            points: 0,
            source: Some(task.source.clone()),
        })
        .search(SearchDocument {
            identities,
            target_paths: vec![relative_root.to_path_buf(), task.source.clone()],
            scopes: vec![scope.clone()],
            tags: Vec::new(),
            text: vec![description.clone()],
        })
        .label(format!("Taskfile task `{}`", task.invocation))
        .description(description)
        .emit(output);
    }
}

fn parse_file(path: &Path, namespace: &str, depth: usize, state: &mut ParseState<'_>) {
    if depth > MAX_INCLUDE_DEPTH {
        invalidate(
            state,
            format!("Taskfile include depth exceeds {MAX_INCLUDE_DEPTH}"),
            path,
        );
        return;
    }
    let Some(path) = bounded_path(path, state) else {
        return;
    };
    if !state.visited.insert((path.clone(), namespace.to_owned())) {
        return;
    }
    if state.files.insert(path.clone()) && state.files.len() > MAX_INCLUDED_FILES {
        invalidate(
            state,
            format!("Taskfile include graph exceeds {MAX_INCLUDED_FILES} files"),
            &path,
        );
        return;
    }
    let contents = match state.context.index.manifests.read(&path) {
        Ok(contents) => contents,
        Err(error) => {
            invalidate(state, error.to_string(), &path);
            return;
        }
    };
    let document = match serde_yaml::from_str::<Value>(&contents) {
        Ok(document) => document,
        Err(error) => {
            invalidate(state, format!("invalid Taskfile YAML: {error}"), &path);
            return;
        }
    };
    let Some(root) = document.as_mapping() else {
        invalidate(state, "Taskfile root must be a mapping", &path);
        return;
    };
    parse_tasks(root, &path, namespace, state);
    parse_includes(root, &path, namespace, depth, state);
}

fn parse_tasks(root: &Mapping, path: &Path, namespace: &str, state: &mut ParseState<'_>) {
    let Some(tasks) = mapping_get(root, "tasks").and_then(Value::as_mapping) else {
        return;
    };
    for (name, value) in tasks {
        let Some(name) = name.as_str().filter(|name| valid_task_name(name)) else {
            state.diagnostics.push(Diagnostic::info(
                TASKFILE,
                "omitted Taskfile task with a non-literal name",
                Some(path.to_path_buf()),
            ));
            continue;
        };
        let mapping = value.as_mapping();
        if mapping
            .and_then(|mapping| mapping_get(mapping, "internal"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            continue;
        }
        let aliases = mapping
            .and_then(|mapping| mapping_get(mapping, "aliases"))
            .map(yaml_strings)
            .unwrap_or_default()
            .into_iter()
            .filter(|alias| valid_task_name(alias))
            .map(|alias| qualify(namespace, &alias))
            .collect::<Vec<_>>();
        let invocation = qualify(namespace, name);
        let description = mapping
            .and_then(|mapping| mapping_get(mapping, "desc"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let requires_confirmation = mapping
            .and_then(|mapping| mapping_get(mapping, "requires"))
            .and_then(Value::as_mapping)
            .and_then(|requires| mapping_get(requires, "vars"))
            .is_some_and(|vars| !yaml_strings(vars).is_empty());
        let task = TaskDefinition {
            name: name.to_owned(),
            invocation: invocation.clone(),
            source: path.to_path_buf(),
            description,
            aliases,
            requires_confirmation,
        };
        if state.tasks.insert(invocation.clone(), task).is_some() {
            invalidate(
                state,
                format!("duplicate Taskfile task `{invocation}` is not statically resolvable"),
                path,
            );
        }
    }
}

fn parse_includes(
    root: &Mapping,
    path: &Path,
    namespace: &str,
    depth: usize,
    state: &mut ParseState<'_>,
) {
    let Some(includes) = mapping_get(root, "includes").and_then(Value::as_mapping) else {
        return;
    };
    for (name, value) in includes {
        let Some(name) = name.as_str().filter(|name| valid_task_name(name)) else {
            invalidate(state, "Taskfile include has a non-literal namespace", path);
            continue;
        };
        let (include_path, flatten, internal, optional) = match value {
            Value::String(path) => (path.as_str(), false, false, false),
            Value::Mapping(options) => {
                let Some(include_path) = mapping_get(options, "taskfile").and_then(Value::as_str)
                else {
                    invalidate(state, "Taskfile include has no literal `taskfile`", path);
                    continue;
                };
                (
                    include_path,
                    mapping_get(options, "flatten").and_then(Value::as_bool) == Some(true),
                    mapping_get(options, "internal").and_then(Value::as_bool) == Some(true),
                    mapping_get(options, "optional").and_then(Value::as_bool) == Some(true),
                )
            }
            _ => {
                invalidate(state, "Taskfile include path is not a literal", path);
                continue;
            }
        };
        if internal {
            continue;
        }
        let Some(include_path) = literal_relative_path(include_path) else {
            invalidate(
                state,
                "Taskfile include path is dynamic or escapes the project",
                path,
            );
            continue;
        };
        let mut absolute = path.parent().unwrap_or(Path::new(".")).join(include_path);
        if absolute.is_dir() {
            let Some(taskfile) = TASKFILE_NAMES
                .iter()
                .map(|name| absolute.join(name))
                .find(|candidate| candidate.is_file())
            else {
                if !optional {
                    invalidate(
                        state,
                        "Taskfile include directory has no Taskfile",
                        &absolute,
                    );
                }
                continue;
            };
            absolute = taskfile;
        }
        if !absolute.is_file() {
            if !optional {
                invalidate(state, "Taskfile include does not exist", &absolute);
            }
            continue;
        }
        let next_namespace = if flatten {
            namespace.to_owned()
        } else if namespace.is_empty() {
            name.to_owned()
        } else {
            format!("{namespace}:{name}")
        };
        parse_file(&absolute, &next_namespace, depth + 1, state);
    }
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_owned()))
}

fn yaml_strings(value: &Value) -> Vec<String> {
    match value {
        Value::String(value) => vec![value.clone()],
        Value::Sequence(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn valid_task_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('_')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | ':' | '.')
        })
}

fn qualify(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_owned()
    } else {
        format!("{namespace}:{name}")
    }
}

fn literal_relative_path(value: &str) -> Option<PathBuf> {
    if value.is_empty() || value.contains(['{', '}', '$', '*', '?', '[', ']']) {
        return None;
    }
    let path = PathBuf::from(value);
    (!path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }))
    .then_some(path)
}

fn bounded_path(path: &Path, state: &mut ParseState<'_>) -> Option<PathBuf> {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let scan_root = std::fs::canonicalize(&state.context.roots.scan_root)
        .unwrap_or_else(|_| state.context.roots.scan_root.clone());
    if !resolved.starts_with(&scan_root) {
        invalidate(
            state,
            "Taskfile include resolves outside the scan root",
            &resolved,
        );
        return None;
    }
    Some(resolved)
}

fn invalidate(state: &mut ParseState<'_>, message: impl Into<String>, source: &Path) {
    state.invalid = true;
    state.diagnostics.push(Diagnostic::warning(
        TASKFILE,
        message,
        Some(source.to_path_buf()),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_taskfile_names_are_explicit() {
        for name in TASKFILE_NAMES {
            assert!(is_taskfile(Path::new(name)), "{name}");
        }
        assert!(!is_taskfile(Path::new("tasks.yml")));
        assert!(!is_taskfile(Path::new("TASKFILE.yml")));
    }

    #[test]
    fn literal_include_paths_reject_templates_and_escape() {
        assert_eq!(
            literal_relative_path("tasks/docs.yml"),
            Some("tasks/docs.yml".into())
        );
        assert!(literal_relative_path("{{.TASKFILE}}.yml").is_none());
        assert!(literal_relative_path("../Taskfile.yml").is_none());
    }
}
