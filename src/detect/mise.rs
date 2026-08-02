use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::candidate::{Evidence, EvidenceKind, PassthroughStyle, SearchDocument};
use crate::diagnostic::Diagnostic;
use crate::registry::{MISE, MISE_SOURCE, MISE_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_INCLUDED_FILES: usize = 64;

pub struct MiseDetector;

#[derive(Clone, Debug)]
struct MiseTask {
    name: String,
    source: PathBuf,
    description: Option<String>,
    aliases: Vec<String>,
}

struct ParseState<'a> {
    context: &'a ScanCtx<'a>,
    tasks: BTreeMap<String, MiseTask>,
    visited: BTreeSet<PathBuf>,
    diagnostics: Vec<Diagnostic>,
    invalid: bool,
}

impl Detector for MiseDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut by_directory = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
        for entry in context.index.all_entries() {
            if entry.file_type != IndexedFileType::File || !is_mise_config(&entry.relative_path) {
                continue;
            }
            let absolute = context.roots.scan_root.join(&entry.relative_path);
            let directory = absolute
                .parent()
                .unwrap_or(&context.roots.scan_root)
                .to_path_buf();
            by_directory.entry(directory).or_default().push(absolute);
        }

        let mut output = Detection::default();
        for (directory, mut configs) in by_directory {
            configs.sort_by_key(|path| config_priority(path));
            detect_configs(context, &directory, &configs, &mut output);
        }
        output
    }
}

fn is_mise_config(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(name, "mise.toml" | ".mise.toml")
                || (name.starts_with("mise.") && name.ends_with(".toml"))
                || (name.starts_with(".mise.") && name.ends_with(".toml"))
        })
}

fn config_priority(path: &Path) -> (bool, bool, String) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    (
        name.contains("local"),
        name.starts_with('.'),
        name.to_owned(),
    )
}

fn detect_configs(
    context: &ScanCtx<'_>,
    directory: &Path,
    configs: &[PathBuf],
    output: &mut Detection,
) {
    let mut state = ParseState {
        context,
        tasks: BTreeMap::new(),
        visited: BTreeSet::new(),
        diagnostics: Vec::new(),
        invalid: false,
    };
    for config in configs {
        parse_config(config, 0, &mut state);
    }
    output.diagnostics.append(&mut state.diagnostics);
    if state.invalid {
        return;
    }

    let scope = directory.file_name().map_or_else(
        || "mise-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    for task in state.tasks.into_values() {
        let Some((selection, points)) = super::task_facade::policy(
            context.invocation.intent,
            task.aliases
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(task.name.as_str())),
            task.name == "default",
        ) else {
            continue;
        };
        let relative_source = task
            .source
            .strip_prefix(&context.roots.scan_root)
            .unwrap_or(&task.source);
        let description = task
            .description
            .clone()
            .unwrap_or_else(|| format!("Declared mise task from {}", relative_source.display()));
        let mut identities = vec![task.name.clone()];
        identities.extend(task.aliases.clone());
        identities.sort();
        identities.dedup();
        CandidateBuilder::project_facade(
            MISE_SOURCE,
            context.invocation.intent,
            directory.to_path_buf(),
            &task.name,
        )
        .action_key(format!(
            "mise:{}:{}",
            directory
                .strip_prefix(&context.roots.scan_root)
                .unwrap_or(directory)
                .to_string_lossy()
                .replace(['/', '\\'], ":"),
            task.name
        ))
        .tool(MISE_TOOL)
        .args([OsString::from("run"), OsString::from(&task.name)])
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
                "{} declares mise task `{}`",
                relative_source.display(),
                task.name
            ),
            points: 0,
            source: Some(relative_source.to_path_buf()),
        })
        .search(SearchDocument {
            identities,
            target_paths: vec![relative_source.to_path_buf()],
            scopes: vec![scope.clone()],
            tags: Vec::new(),
            text: vec![description.clone()],
        })
        .label(format!("mise task `{}`", task.name))
        .description(description)
        .emit(output);
    }
}

fn parse_config(path: &Path, depth: usize, state: &mut ParseState<'_>) {
    if depth > MAX_INCLUDE_DEPTH {
        invalidate(
            state,
            format!("mise include depth exceeds {MAX_INCLUDE_DEPTH}"),
            path,
        );
        return;
    }
    let Some(path) = bounded_path(path, state) else {
        return;
    };
    if !state.visited.insert(path.clone()) {
        return;
    }
    if state.visited.len() > MAX_INCLUDED_FILES {
        invalidate(
            state,
            format!("mise include graph exceeds {MAX_INCLUDED_FILES} files"),
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
    let document = match toml::from_str::<toml::Value>(&contents) {
        Ok(document) => document,
        Err(error) => {
            invalidate(state, format!("invalid mise TOML: {error}"), &path);
            return;
        }
    };
    if let Some(tasks) = document.get("tasks").and_then(toml::Value::as_table) {
        for (name, value) in tasks {
            parse_task(name, value, &path, state);
        }
    }
    let includes = document
        .get("task_config")
        .and_then(toml::Value::as_table)
        .and_then(|config| config.get("includes"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for include in includes {
        let Some(relative) = literal_relative_path(&include) else {
            invalidate(
                state,
                "mise task include is dynamic or escapes the project",
                &path,
            );
            continue;
        };
        parse_config(
            &path.parent().unwrap_or(Path::new(".")).join(relative),
            depth + 1,
            state,
        );
    }
}

fn parse_task(name: &str, value: &toml::Value, path: &Path, state: &mut ParseState<'_>) {
    if !valid_task_name(name) {
        state.diagnostics.push(Diagnostic::info(
            MISE,
            format!("omitted mise task with unsupported name `{name}`"),
            Some(path.to_path_buf()),
        ));
        return;
    }
    let (hidden, description, aliases, executable) = match value {
        toml::Value::String(_) | toml::Value::Array(_) => (false, None, Vec::new(), true),
        toml::Value::Table(table) => {
            let hidden = table.get("hide").and_then(toml::Value::as_bool) == Some(true);
            let description = table
                .get("description")
                .and_then(toml::Value::as_str)
                .map(str::to_owned);
            let aliases = table
                .get("alias")
                .or_else(|| table.get("aliases"))
                .map(toml_strings)
                .unwrap_or_default();
            let executable = table.contains_key("run") || table.contains_key("depends");
            (hidden, description, aliases, executable)
        }
        _ => (false, None, Vec::new(), false),
    };
    if hidden || !executable {
        return;
    }
    let aliases = aliases
        .into_iter()
        .filter(|alias| valid_task_name(alias))
        .collect();
    state.tasks.insert(
        name.to_owned(),
        MiseTask {
            name: name.to_owned(),
            source: path.to_path_buf(),
            description,
            aliases,
        },
    );
}

fn toml_strings(value: &toml::Value) -> Vec<String> {
    match value {
        toml::Value::String(value) => vec![value.clone()],
        toml::Value::Array(values) => values
            .iter()
            .filter_map(toml::Value::as_str)
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
            "mise include resolves outside the scan root",
            &resolved,
        );
        return None;
    }
    Some(resolved)
}

fn invalidate(state: &mut ParseState<'_>, message: impl Into<String>, source: &Path) {
    state.invalid = true;
    state.diagnostics.push(Diagnostic::warning(
        MISE,
        message,
        Some(source.to_path_buf()),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mise_config_names_include_environment_and_local_variants() {
        for name in [
            "mise.toml",
            ".mise.toml",
            "mise.local.toml",
            ".mise.dev.toml",
        ] {
            assert!(is_mise_config(Path::new(name)), "{name}");
        }
        assert!(!is_mise_config(Path::new("mise.lock")));
        assert!(!is_mise_config(Path::new("my-mise.toml")));
    }
}
