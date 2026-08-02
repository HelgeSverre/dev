use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::candidate::{Evidence, EvidenceKind, PassthroughStyle, SearchDocument};
use crate::diagnostic::Diagnostic;
use crate::registry::{JAKE, JAKE_SOURCE, JAKE_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_INCLUDED_FILES: usize = 64;

pub struct JakeDetector;

#[derive(Clone, Debug)]
struct JakeTask {
    name: String,
    invocation: String,
    source: PathBuf,
    description: Option<String>,
    aliases: Vec<String>,
    is_default: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Import {
    path: PathBuf,
    namespace: Option<String>,
}

struct ParseState<'a> {
    context: &'a ScanCtx<'a>,
    tasks: BTreeMap<String, JakeTask>,
    visited: BTreeSet<(PathBuf, String)>,
    files: BTreeSet<PathBuf>,
    diagnostics: Vec<Diagnostic>,
    invalid: bool,
}

impl Detector for JakeDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut roots = context
            .index
            .all_entries()
            .filter(|entry| {
                entry.file_type == IndexedFileType::File
                    && entry
                        .relative_path
                        .file_name()
                        .is_some_and(|name| name == "Jakefile")
            })
            .map(|entry| context.roots.scan_root.join(&entry.relative_path))
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();

        let mut output = Detection::default();
        for root in roots {
            let directory = root
                .parent()
                .unwrap_or(&context.roots.scan_root)
                .to_path_buf();
            detect_file(context, &root, &directory, &mut output);
        }
        output
    }
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
    parse_file(root_file, "", true, 0, &mut state);
    output.diagnostics.append(&mut state.diagnostics);
    if state.invalid {
        return;
    }

    let relative_root = root_file
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(root_file);
    let scope = directory.file_name().map_or_else(
        || "jake-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    for task in state.tasks.into_values() {
        let Some((selection, points)) = super::task_facade::policy(
            context.invocation.intent,
            task.aliases
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(task.name.as_str())),
            task.is_default,
        ) else {
            continue;
        };
        let description = task
            .description
            .clone()
            .unwrap_or_else(|| format!("Public Jake task from {}", relative_root.display()));
        let mut identities = vec![task.name.clone(), task.invocation.clone()];
        identities.extend(task.aliases.clone());
        identities.sort();
        identities.dedup();
        CandidateBuilder::project_facade(
            JAKE_SOURCE,
            context.invocation.intent,
            directory.to_path_buf(),
            &task.name,
        )
        .action_key(format!(
            "jake:{}:{}",
            relative_root.to_string_lossy().replace(['/', '\\'], ":"),
            task.invocation
        ))
        .tool(JAKE_TOOL)
        .args([
            OsString::from("-f"),
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
                "{} declares Jake task `{}`",
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
        .label(format!("Jake task `{}`", task.invocation))
        .description(description)
        .emit(output);
    }
}

fn parse_file(path: &Path, namespace: &str, root: bool, depth: usize, state: &mut ParseState<'_>) {
    if depth > MAX_INCLUDE_DEPTH {
        invalidate(
            state,
            format!("Jake import depth exceeds {MAX_INCLUDE_DEPTH}"),
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
            format!("Jake import graph exceeds {MAX_INCLUDED_FILES} files"),
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
    let imports = parse_contents(&contents, &path, namespace, root, state);
    for import in imports {
        let next_namespace = match (namespace.is_empty(), import.namespace) {
            (_, None) => namespace.to_owned(),
            (true, Some(imported)) => imported,
            (false, Some(imported)) => format!("{namespace}.{imported}"),
        };
        parse_file(
            &path.parent().unwrap_or(Path::new(".")).join(import.path),
            &next_namespace,
            false,
            depth + 1,
            state,
        );
    }
}

fn parse_contents(
    contents: &str,
    path: &Path,
    namespace: &str,
    root: bool,
    state: &mut ParseState<'_>,
) -> Vec<Import> {
    let mut imports = Vec::new();
    let mut docs = Vec::new();
    let mut pending_description = None;
    let mut pending_default = false;
    let mut first_root_task = true;
    let mut current_task = None::<String>;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            docs.clear();
            pending_description = None;
            current_task = None;
            continue;
        }
        if let Some(comment) = trimmed.strip_prefix('#') {
            if !line.starts_with(char::is_whitespace) {
                let comment = comment.trim();
                if !comment.is_empty() {
                    docs.push(comment.to_owned());
                }
            }
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            if let Some(description) = trimmed
                .strip_prefix("@desc ")
                .and_then(parse_quoted_literal)
            {
                if let Some(task) = current_task
                    .as_ref()
                    .and_then(|name| state.tasks.get_mut(name))
                {
                    task.description = Some(description);
                }
            }
            continue;
        }
        current_task = None;
        if let Some(rest) = trimmed.strip_prefix("@import ") {
            match parse_import(rest) {
                Some(import) => imports.push(import),
                None => invalidate(state, "Jake import is not a supported literal", path),
            }
            docs.clear();
            pending_description = None;
            continue;
        }
        if trimmed == "@default" {
            pending_default = true;
            continue;
        }
        if let Some(description) = trimmed
            .strip_prefix("@desc ")
            .and_then(parse_quoted_literal)
        {
            pending_description = Some(description);
            continue;
        }
        if trimmed.starts_with('@') {
            continue;
        }
        let Some(header) = parse_task_header(trimmed) else {
            docs.clear();
            pending_description = None;
            pending_default = false;
            continue;
        };
        if header.required_parameters {
            state.diagnostics.push(Diagnostic::info(
                JAKE,
                format!(
                    "omitted Jake task `{}` because it requires parameters",
                    header.name
                ),
                Some(path.to_path_buf()),
            ));
            docs.clear();
            pending_description = None;
            pending_default = false;
            continue;
        }
        let invocation = qualify(namespace, &header.name);
        let aliases = header
            .aliases
            .into_iter()
            .map(|alias| qualify(namespace, &alias))
            .collect();
        let description = pending_description
            .take()
            .or_else(|| (!docs.is_empty()).then(|| docs.join(" ")));
        docs.clear();
        let task = JakeTask {
            name: header.name,
            invocation: invocation.clone(),
            source: path.to_path_buf(),
            description,
            aliases,
            is_default: pending_default || (root && first_root_task),
        };
        pending_default = false;
        if root {
            first_root_task = false;
        }
        if state.tasks.insert(invocation.clone(), task).is_some() {
            invalidate(
                state,
                format!("duplicate Jake task `{invocation}` is not statically resolvable"),
                path,
            );
        }
        current_task = Some(invocation);
    }
    imports
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TaskHeader {
    name: String,
    aliases: Vec<String>,
    required_parameters: bool,
}

fn parse_task_header(line: &str) -> Option<TaskHeader> {
    if line.starts_with("file ") || line.contains(":=") {
        return None;
    }
    let line = line.strip_prefix("task ").unwrap_or(line);
    let (left, _) = line.split_once(':')?;
    let mut aliases = left.split('|');
    let primary = aliases.next()?.trim();
    let mut tokens = super::just::split_header(primary);
    let name = tokens.first()?.clone();
    if !valid_task_name(&name) {
        return None;
    }
    tokens.remove(0);
    let required_parameters = tokens.iter().any(|parameter| !parameter.contains('='));
    let aliases = aliases
        .map(str::trim)
        .filter(|alias| valid_task_name(alias))
        .map(str::to_owned)
        .collect();
    Some(TaskHeader {
        name,
        aliases,
        required_parameters,
    })
}

fn parse_import(value: &str) -> Option<Import> {
    let (path, rest) = take_quoted_literal(value)?;
    let path = literal_relative_path(&path)?;
    let tokens = rest.split_ascii_whitespace().collect::<Vec<_>>();
    let namespace = match tokens.as_slice() {
        [] | ["rooted"] => None,
        ["as", namespace] | ["as", namespace, "rooted"] if valid_task_name(namespace) => {
            Some((*namespace).to_owned())
        }
        _ => return None,
    };
    Some(Import { path, namespace })
}

fn parse_quoted_literal(value: &str) -> Option<String> {
    let (literal, rest) = take_quoted_literal(value)?;
    rest.trim().is_empty().then_some(literal)
}

fn take_quoted_literal(value: &str) -> Option<(String, &str)> {
    let value = value.trim();
    let quote = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))?;
    let tail = &value[quote.len_utf8()..];
    let end = tail.find(quote)?;
    Some((tail[..end].to_owned(), &tail[end + quote.len_utf8()..]))
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

fn valid_task_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('_')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn qualify(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_owned()
    } else {
        format!("{namespace}.{name}")
    }
}

fn bounded_path(path: &Path, state: &mut ParseState<'_>) -> Option<PathBuf> {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let scan_root = std::fs::canonicalize(&state.context.roots.scan_root)
        .unwrap_or_else(|_| state.context.roots.scan_root.clone());
    if !resolved.starts_with(&scan_root) {
        invalidate(
            state,
            "Jake import resolves outside the scan root",
            &resolved,
        );
        return None;
    }
    Some(resolved)
}

fn invalidate(state: &mut ParseState<'_>, message: impl Into<String>, source: &Path) {
    state.invalid = true;
    state.diagnostics.push(Diagnostic::warning(
        JAKE,
        message,
        Some(source.to_path_buf()),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_header_supports_aliases_defaults_and_required_params() {
        assert_eq!(
            parse_task_header("task build | b | compile:"),
            Some(TaskHeader {
                name: "build".to_owned(),
                aliases: vec!["b".to_owned(), "compile".to_owned()],
                required_parameters: false,
            })
        );
        assert!(
            !parse_task_header("task deploy env=\"staging\":")
                .expect("header")
                .required_parameters
        );
        assert!(
            parse_task_header("task deploy env:")
                .expect("header")
                .required_parameters
        );
        assert!(parse_task_header("file app: source").is_none());
    }

    #[test]
    fn imports_are_literal_and_namespaced() {
        assert_eq!(
            parse_import("\"jake/test.jake\" as checks rooted"),
            Some(Import {
                path: "jake/test.jake".into(),
                namespace: Some("checks".to_owned()),
            })
        );
        assert!(parse_import("path_var").is_none());
        assert!(parse_import("\"../Jakefile\"").is_none());
    }
}
