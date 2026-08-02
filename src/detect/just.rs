use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::candidate::{
    Evidence, EvidenceKind, Lifecycle, PassthroughStyle, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{JUST, JUST_SOURCE, JUST_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_INCLUDED_FILES: usize = 64;

pub struct JustDetector;

#[derive(Clone, Debug)]
struct Recipe {
    name: String,
    invocation: String,
    source: PathBuf,
    description: Option<String>,
    aliases: Vec<String>,
    is_default: bool,
}

#[derive(Clone, Debug)]
struct Alias {
    name: String,
    target: String,
    private: bool,
}

#[derive(Clone, Debug)]
enum IncludeKind {
    Import,
    Module(String),
}

#[derive(Clone, Debug)]
struct Include {
    kind: IncludeKind,
    path: PathBuf,
}

struct ParseState<'a> {
    context: &'a ScanCtx<'a>,
    root_file: &'a Path,
    recipes: BTreeMap<String, Recipe>,
    aliases: Vec<Alias>,
    visited: BTreeSet<(PathBuf, String)>,
    files: BTreeSet<PathBuf>,
    diagnostics: Vec<Diagnostic>,
    invalid: bool,
}

impl Detector for JustDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut by_directory = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
        for entry in context.index.all_entries() {
            if entry.file_type != IndexedFileType::File || !is_justfile(&entry.relative_path) {
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
        for (directory, mut files) in by_directory {
            files.sort();
            let ambiguous_file = files.len() > 1;
            if ambiguous_file {
                output.diagnostics.push(Diagnostic::warning(
                    JUST,
                    format!(
                        "multiple accepted Justfiles in `{}`; recipes require confirmation",
                        directory.display()
                    ),
                    Some(directory.clone()),
                ));
            }
            for file in files {
                detect_file(context, &directory, &file, ambiguous_file, &mut output);
            }
        }
        output
    }
}

fn is_justfile(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == ".justfile" || name.eq_ignore_ascii_case("justfile"))
}

fn detect_file(
    context: &ScanCtx<'_>,
    directory: &Path,
    justfile: &Path,
    ambiguous_file: bool,
    output: &mut Detection,
) {
    let mut state = ParseState {
        context,
        root_file: justfile,
        recipes: BTreeMap::new(),
        aliases: Vec::new(),
        visited: BTreeSet::new(),
        files: BTreeSet::new(),
        diagnostics: Vec::new(),
        invalid: false,
    };
    parse_file(justfile, "", true, 0, &mut state);
    attach_aliases(&mut state);
    output.diagnostics.append(&mut state.diagnostics);
    if state.invalid {
        return;
    }

    let relative_file = justfile
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(justfile);
    let scope = directory.file_name().map_or_else(
        || "just-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    for recipe in state.recipes.into_values() {
        let Some((selection, points)) = recipe_policy(
            context.invocation.intent,
            &recipe.name,
            &recipe.aliases,
            recipe.is_default,
        ) else {
            continue;
        };
        let selection = if ambiguous_file {
            SelectionPolicy::Confirm
        } else {
            selection
        };
        let long_running = context.invocation.intent == Intent::Run
            && recipe
                .aliases
                .iter()
                .chain(std::iter::once(&recipe.name))
                .any(|name| matches!(name.as_str(), "dev" | "start" | "serve" | "watch"));
        let description = recipe.description.clone().unwrap_or_else(|| {
            format!("Public zero-arity recipe from {}", relative_file.display())
        });
        let mut identities = vec![recipe.name.clone(), recipe.invocation.clone()];
        identities.extend(recipe.aliases.clone());
        identities.sort();
        identities.dedup();
        CandidateBuilder::project_facade(
            JUST_SOURCE,
            context.invocation.intent,
            directory.to_path_buf(),
            &recipe.name,
        )
        .action_key(format!(
            "just:{}:recipe:{}",
            relative_file.to_string_lossy().replace(['/', '\\'], ":"),
            recipe.invocation
        ))
        .tool(JUST_TOOL)
        .args([
            OsString::from("--justfile"),
            justfile.as_os_str().to_owned(),
            OsString::from(&recipe.invocation),
        ])
        .cwd(directory.to_path_buf())
        .passthrough(PassthroughStyle::Append)
        .selection(selection)
        .base_points(points)
        .lifecycle(if long_running {
            Lifecycle::LongRunning
        } else {
            Lifecycle::Finite
        })
        .evidence(Evidence {
            kind: EvidenceKind::Manifest,
            reason: format!(
                "{} declares public zero-arity recipe `{}`",
                recipe.source.display(),
                recipe.invocation
            ),
            points: 0,
            source: Some(recipe.source.clone()),
        })
        .search(SearchDocument {
            identities,
            target_paths: vec![relative_file.to_path_buf(), recipe.source.clone()],
            scopes: vec![scope.clone()],
            tags: Vec::new(),
            text: vec![description.clone()],
        })
        .label(format!("Just recipe `{}`", recipe.invocation))
        .description(description)
        .emit(output);
    }
}

fn recipe_policy(
    intent: Intent,
    name: &str,
    aliases: &[String],
    is_default: bool,
) -> Option<(SelectionPolicy, i32)> {
    let names = aliases
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(name));
    let canonical_points = names
        .clone()
        .filter_map(|name| canonical_points(intent, name))
        .max();
    if let Some(points) = canonical_points {
        return Some((SelectionPolicy::Automatic, points));
    }
    match intent {
        Intent::Run => {
            let canonical_other = names.clone().any(|name| {
                canonical_for(Intent::Build, name) || canonical_for(Intent::Test, name)
            });
            if is_default && !canonical_other {
                Some((SelectionPolicy::Automatic, 85))
            } else {
                Some((SelectionPolicy::ExplicitHint, 15))
            }
        }
        Intent::Build | Intent::Test => None,
    }
}

fn canonical_for(intent: Intent, name: &str) -> bool {
    canonical_points(intent, name).is_some()
}

fn canonical_points(intent: Intent, name: &str) -> Option<i32> {
    match intent {
        Intent::Run => match name {
            "dev" => Some(95),
            "run" | "start" => Some(90),
            "serve" | "watch" => Some(75),
            _ => None,
        },
        Intent::Build => match name {
            "build" => Some(95),
            "all" => Some(85),
            "compile" | "bundle" => Some(75),
            _ => None,
        },
        Intent::Test => match name {
            "test" => Some(95),
            "check" | "verify" => Some(75),
            _ => None,
        },
    }
}

fn parse_file(path: &Path, namespace: &str, root: bool, depth: usize, state: &mut ParseState<'_>) {
    if depth > MAX_INCLUDE_DEPTH {
        invalidate(
            state,
            format!("Just include depth exceeds {MAX_INCLUDE_DEPTH}"),
            path,
        );
        return;
    }
    let Some(path) = bounded_include_path(path, state) else {
        return;
    };
    if !state.visited.insert((path.clone(), namespace.to_owned())) {
        return;
    }
    if state.files.insert(path.clone()) && state.files.len() > MAX_INCLUDED_FILES {
        invalidate(
            state,
            format!("Just include graph exceeds {MAX_INCLUDED_FILES} files"),
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
    let parsed = parse_contents(&contents, &path, namespace, root, state);
    for include in parsed {
        let next_namespace = match include.kind {
            IncludeKind::Import => namespace.to_owned(),
            IncludeKind::Module(module) if namespace.is_empty() => module,
            IncludeKind::Module(module) => format!("{namespace}::{module}"),
        };
        parse_file(
            &path.parent().unwrap_or(Path::new(".")).join(include.path),
            &next_namespace,
            false,
            depth + 1,
            state,
        );
    }
}

fn bounded_include_path(path: &Path, state: &mut ParseState<'_>) -> Option<PathBuf> {
    if !path.is_absolute()
        && path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        invalidate(state, "Just include path escapes its project", path);
        return None;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        state
            .root_file
            .parent()
            .unwrap_or(&state.context.roots.scan_root)
            .join(path)
    };
    let resolved = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    let scan_root = std::fs::canonicalize(&state.context.roots.scan_root)
        .unwrap_or_else(|_| state.context.roots.scan_root.clone());
    if !resolved.starts_with(&scan_root) {
        invalidate(
            state,
            "Just include resolves outside the scan root",
            &resolved,
        );
        return None;
    }
    Some(resolved)
}

fn parse_contents(
    contents: &str,
    path: &Path,
    namespace: &str,
    root: bool,
    state: &mut ParseState<'_>,
) -> Vec<Include> {
    let mut includes = Vec::new();
    let mut pending_private = false;
    let mut docs = Vec::new();
    let mut root_default_seen = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            pending_private = false;
            docs.clear();
            continue;
        }
        if let Some(comment) = trimmed.strip_prefix('#') {
            let comment = comment.trim_start_matches('#').trim();
            if !comment.is_empty() {
                docs.push(comment.to_owned());
            }
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            pending_private = trimmed[1..trimmed.len() - 1]
                .split(',')
                .any(|attribute| attribute.trim().starts_with("private"));
            continue;
        }
        if trimmed.starts_with("import") || trimmed.starts_with("mod ") {
            match parse_include(trimmed, path) {
                Ok(include) => includes.push(include),
                Err(message) => invalidate(state, message, path),
            }
            pending_private = false;
            docs.clear();
            continue;
        }
        if let Some((name, target)) = parse_alias(trimmed) {
            let name = qualify(namespace, &name);
            let target = qualify(namespace, &target);
            state.aliases.push(Alias {
                private: pending_private
                    || name
                        .rsplit("::")
                        .next()
                        .is_some_and(|name| name.starts_with('_')),
                name,
                target,
            });
            pending_private = false;
            docs.clear();
            continue;
        }
        let Some((name, parameters)) = parse_recipe_header(trimmed) else {
            pending_private = false;
            docs.clear();
            continue;
        };
        let is_default = root && !root_default_seen;
        if root {
            root_default_seen = true;
        }
        let private = pending_private || name.starts_with('_');
        pending_private = false;
        if private {
            docs.clear();
            continue;
        }
        if requires_arguments(&parameters) {
            state.diagnostics.push(Diagnostic::info(
                JUST,
                format!("omitted Just recipe `{name}` because it requires positional arguments"),
                Some(path.to_path_buf()),
            ));
            docs.clear();
            continue;
        }
        let invocation = qualify(namespace, &name);
        let recipe = Recipe {
            name,
            invocation: invocation.clone(),
            source: path.to_path_buf(),
            description: (!docs.is_empty()).then(|| docs.join(" ")),
            aliases: Vec::new(),
            is_default,
        };
        docs.clear();
        if state.recipes.insert(invocation.clone(), recipe).is_some() {
            invalidate(
                state,
                format!("duplicate Just recipe `{invocation}` is not statically resolvable"),
                path,
            );
        }
    }
    includes
}

fn parse_include(line: &str, source: &Path) -> Result<Include, String> {
    if let Some(rest) = line.strip_prefix("import ") {
        return parse_literal_path(rest).map(|path| Include {
            kind: IncludeKind::Import,
            path,
        });
    }
    let Some(rest) = line.strip_prefix("mod ") else {
        return Err(format!(
            "unsupported dynamic Just include in {}",
            source.display()
        ));
    };
    let mut tokens = split_header(rest);
    if tokens.len() != 2 || !is_recipe_name(&tokens[0]) {
        return Err(format!(
            "unsupported non-literal Just module in {}",
            source.display()
        ));
    }
    let path = parse_literal_path(&tokens.remove(1))?;
    Ok(Include {
        kind: IncludeKind::Module(tokens.remove(0)),
        path,
    })
}

fn parse_literal_path(value: &str) -> Result<PathBuf, String> {
    let value = value.trim();
    let Some(quote) = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))
    else {
        return Err("Just include path is not a quoted literal".to_owned());
    };
    if value.len() < 2 || !value.ends_with(quote) {
        return Err("Just include path has an unterminated literal".to_owned());
    }
    let literal = &value[quote.len_utf8()..value.len() - quote.len_utf8()];
    if literal.is_empty() || literal.contains(['\n', '\r']) {
        return Err("Just include path is empty or invalid".to_owned());
    }
    let path = PathBuf::from(literal);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err("Just include path escapes the scan root".to_owned());
    }
    Ok(path)
}

fn parse_alias(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("alias ")?;
    let (name, target) = rest.split_once(":=")?;
    let name = name.trim();
    let target = target.trim();
    (is_recipe_name(name) && is_qualified_recipe_name(target))
        .then(|| (name.to_owned(), target.to_owned()))
}

fn parse_recipe_header(line: &str) -> Option<(String, Vec<String>)> {
    if line.contains(":=") || line.ends_with('\\') {
        return None;
    }
    let (left, _) = line.split_once(':')?;
    let mut tokens = split_header(left.trim_start_matches('@').trim());
    let name = tokens.first()?.clone();
    if !is_recipe_name(&name) {
        return None;
    }
    tokens.remove(0);
    Some((name, tokens))
}

fn split_header(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut nesting = 0_u16;
    for character in value.chars() {
        match character {
            '\'' | '"' if quote == Some(character) => {
                quote = None;
                current.push(character);
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(character);
                current.push(character);
            }
            '(' | '[' | '{' if quote.is_none() => {
                nesting = nesting.saturating_add(1);
                current.push(character);
            }
            ')' | ']' | '}' if quote.is_none() => {
                nesting = nesting.saturating_sub(1);
                current.push(character);
            }
            character if character.is_whitespace() && quote.is_none() && nesting == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn requires_arguments(parameters: &[String]) -> bool {
    parameters.iter().any(|parameter| {
        let parameter = parameter.trim_start_matches('$');
        parameter.starts_with('+') || (!parameter.starts_with('*') && !parameter.contains('='))
    })
}

fn is_recipe_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn is_qualified_recipe_name(name: &str) -> bool {
    name.split("::").all(is_recipe_name)
}

fn qualify(namespace: &str, name: &str) -> String {
    if namespace.is_empty() || name.contains("::") {
        name.to_owned()
    } else {
        format!("{namespace}::{name}")
    }
}

fn attach_aliases(state: &mut ParseState<'_>) {
    for alias in &state.aliases {
        if alias.private
            || alias
                .name
                .rsplit("::")
                .next()
                .is_some_and(|name| name.starts_with('_'))
        {
            continue;
        }
        let Some(recipe) = state.recipes.get_mut(&alias.target) else {
            state.diagnostics.push(Diagnostic::warning(
                JUST,
                format!(
                    "Just alias `{}` has unresolved target `{}`",
                    alias.name, alias.target
                ),
                Some(state.root_file.to_path_buf()),
            ));
            continue;
        };
        recipe.aliases.push(alias.name.clone());
    }
    for recipe in state.recipes.values_mut() {
        recipe.aliases.sort();
        recipe.aliases.dedup();
    }
}

fn invalidate(state: &mut ParseState<'_>, message: impl Into<String>, source: &Path) {
    state.invalid = true;
    state.diagnostics.push(Diagnostic::warning(
        JUST,
        message,
        Some(source.to_path_buf()),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_headers_distinguish_zero_and_required_arity() {
        for header in ["test:", "test arg='value':", "test *args:"] {
            let (_, parameters) = parse_recipe_header(header).expect("literal recipe header");
            assert!(!requires_arguments(&parameters), "{header}");
        }
        for header in ["test arg:", "test +args:"] {
            let (_, parameters) = parse_recipe_header(header).expect("literal recipe header");
            assert!(requires_arguments(&parameters), "{header}");
        }
    }

    #[test]
    fn accepted_justfile_spellings_are_bounded() {
        assert!(is_justfile(Path::new("Justfile")));
        assert!(is_justfile(Path::new("JUSTFILE")));
        assert!(is_justfile(Path::new(".justfile")));
        assert!(!is_justfile(Path::new("project.just")));
        assert!(!is_justfile(Path::new(".JUSTFILE")));
    }
}
