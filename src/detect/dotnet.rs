use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::candidate::{
    Evidence, EvidenceKind, Lifecycle, PassthroughStyle, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{
    WorkspaceContribution, WorkspaceContributor, DOTNET, DOTNET_SOURCE, DOTNET_TOOL,
};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

const PROJECT_EXTENSIONS: &[&str] = &["csproj", "fsproj", "vbproj"];
const SOLUTION_EXTENSIONS: &[&str] = &["sln", "slnx", "slnf"];

pub struct DotnetDetector;
pub struct DotnetWorkspaceContributor;

impl WorkspaceContributor for DotnetWorkspaceContributor {
    fn is_workspace(&self, root: &Path) -> bool {
        root_solution_files(root).next().is_some()
    }

    fn scan_contribution(&self, root: &Path) -> WorkspaceContribution {
        let mut includes = Vec::new();
        for solution in root_solution_files(root) {
            let contents = std::fs::read_to_string(&solution).unwrap_or_default();
            let extension = solution
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let members = match extension.as_str() {
                "sln" => quoted_project_paths(&contents),
                "slnx" => xml_project_paths(&contents),
                "slnf" => json_solution_paths(&contents),
                _ => Vec::new(),
            };
            includes.extend(
                members
                    .into_iter()
                    .map(|member| normalize_member(Path::new(""), &member))
                    .filter(|member| safe_relative(member))
                    .map(|member| member.to_string_lossy().replace('\\', "/")),
            );
        }
        includes.sort();
        includes.dedup();
        WorkspaceContribution {
            includes,
            excludes: Vec::new(),
        }
    }
}

fn root_solution_files(root: &Path) -> impl Iterator<Item = PathBuf> {
    let mut paths = std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    SOLUTION_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
                })
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.into_iter()
}

#[derive(Clone, Debug)]
struct DotnetProject {
    path: PathBuf,
    name: String,
    runnable: bool,
    test: bool,
}

#[derive(Clone, Debug)]
struct DotnetSolution {
    path: PathBuf,
    members: Vec<PathBuf>,
}

impl Detector for DotnetDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut output = Detection::default();
        let mut projects = BTreeMap::new();
        let mut solutions = Vec::new();
        for entry in context.index.all_entries() {
            if entry.file_type != IndexedFileType::File {
                continue;
            }
            let extension = entry
                .relative_path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if PROJECT_EXTENSIONS.contains(&extension.as_str()) {
                if let Some(project) = parse_project(context, &entry.relative_path, &mut output) {
                    projects.insert(entry.relative_path.clone(), project);
                }
            } else if SOLUTION_EXTENSIONS.contains(&extension.as_str()) {
                if let Some(solution) = parse_solution(context, &entry.relative_path, &mut output) {
                    solutions.push(solution);
                }
            }
        }
        solutions.sort_by(|left, right| left.path.cmp(&right.path));
        solutions.dedup_by(|left, right| left.path == right.path);

        for solution in &solutions {
            for member in &solution.members {
                if projects.contains_key(member) {
                    continue;
                }
                if let Some(project) = parse_project(context, member, &mut output) {
                    projects.insert(member.clone(), project);
                }
            }
        }
        emit_candidates(context, &solutions, &projects, &mut output);
        output
    }
}

fn parse_project(
    context: &ScanCtx<'_>,
    relative: &Path,
    output: &mut Detection,
) -> Option<DotnetProject> {
    if !safe_relative(relative) {
        return None;
    }
    let absolute = context.roots.scan_root.join(relative);
    let contents = match context.index.manifests.read(&absolute) {
        Ok(contents) => contents,
        Err(error) => {
            output.diagnostics.push(Diagnostic::warning(
                DOTNET,
                error.to_string(),
                Some(absolute),
            ));
            return None;
        }
    };
    if !contents.contains("<Project") {
        return None;
    }
    let lowercase = contents.to_ascii_lowercase();
    let output_type = first_tag_value(&lowercase, "outputtype");
    let runnable = matches!(output_type.as_deref(), Some("exe" | "winexe"))
        || lowercase.contains("microsoft.net.sdk.web")
        || lowercase.contains("microsoft.net.sdk.worker");
    let test = first_tag_value(&lowercase, "istestproject").as_deref() == Some("true")
        || lowercase.contains("microsoft.net.test.sdk");
    Some(DotnetProject {
        path: relative.to_path_buf(),
        name: relative.file_stem().map_or_else(
            || "dotnet-project".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        ),
        runnable,
        test,
    })
}

fn parse_solution(
    context: &ScanCtx<'_>,
    relative: &Path,
    output: &mut Detection,
) -> Option<DotnetSolution> {
    let absolute = context.roots.scan_root.join(relative);
    let contents = match context.index.manifests.read(&absolute) {
        Ok(contents) => contents,
        Err(error) => {
            output.diagnostics.push(Diagnostic::warning(
                DOTNET,
                error.to_string(),
                Some(absolute),
            ));
            return None;
        }
    };
    let extension = relative
        .extension()
        .and_then(|extension| extension.to_str())?;
    let base = relative.parent().unwrap_or(Path::new(""));
    let raw_members = match extension.to_ascii_lowercase().as_str() {
        "sln" => quoted_project_paths(&contents),
        "slnx" => xml_project_paths(&contents),
        "slnf" => json_solution_paths(&contents),
        _ => Vec::new(),
    };
    let mut members = raw_members
        .into_iter()
        .map(|path| normalize_member(base, &path))
        .filter(|path| safe_relative(path))
        .collect::<Vec<_>>();
    members.sort();
    members.dedup();
    Some(DotnetSolution {
        path: relative.to_path_buf(),
        members,
    })
}

fn emit_candidates(
    context: &ScanCtx<'_>,
    solutions: &[DotnetSolution],
    projects: &BTreeMap<PathBuf, DotnetProject>,
    output: &mut Detection,
) {
    match context.invocation.intent {
        Intent::Build => {
            for solution in solutions {
                emit_solution(context, solution, "build", 90, output);
            }
            let selection = if solutions.is_empty() {
                SelectionPolicy::Automatic
            } else {
                SelectionPolicy::ExplicitHint
            };
            for project in projects.values() {
                emit_project_action(context, project, "build", selection, output);
            }
        }
        Intent::Test => {
            let mut covered = BTreeSet::new();
            for solution in solutions {
                if solution
                    .members
                    .iter()
                    .any(|member| projects.get(member).is_some_and(|project| project.test))
                {
                    covered.extend(solution.members.iter().cloned());
                    emit_solution(context, solution, "test", 90, output);
                }
            }
            for project in projects.values().filter(|project| project.test) {
                emit_project_action(
                    context,
                    project,
                    "test",
                    if covered.contains(&project.path) {
                        SelectionPolicy::ExplicitHint
                    } else {
                        SelectionPolicy::Automatic
                    },
                    output,
                );
            }
        }
        Intent::Run => {
            for project in projects.values().filter(|project| project.runnable) {
                emit_project_action(context, project, "run", SelectionPolicy::Automatic, output);
            }
        }
    }
}

fn emit_solution(
    context: &ScanCtx<'_>,
    solution: &DotnetSolution,
    action: &str,
    points: i32,
    output: &mut Detection,
) {
    let relative_directory = solution.path.parent().unwrap_or(Path::new(""));
    let directory = if relative_directory.as_os_str().is_empty() {
        context.roots.scan_root.clone()
    } else {
        context.roots.scan_root.join(relative_directory)
    };
    let argument = solution
        .path
        .file_name()
        .map_or_else(|| solution.path.as_os_str().to_owned(), OsString::from);
    let name = solution.path.file_stem().map_or_else(
        || "solution".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    CandidateBuilder::tool_default(
        DOTNET_SOURCE,
        context.invocation.intent,
        directory.clone(),
        action,
    )
    .action_key(format!(
        "dotnet:solution:{}:{action}",
        solution.path.to_string_lossy().replace(['/', '\\'], ":")
    ))
    .tool(DOTNET_TOOL)
    .args([OsString::from(action), argument])
    .cwd(directory)
    .passthrough(PassthroughStyle::Append)
    .selection(SelectionPolicy::Automatic)
    .base_points(points)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: format!(
            "{} declares {} project(s)",
            solution.path.display(),
            solution.members.len()
        ),
        points: 0,
        source: Some(solution.path.clone()),
    })
    .search(SearchDocument {
        identities: vec![action.to_owned(), name.clone()],
        target_paths: std::iter::once(solution.path.clone())
            .chain(solution.members.iter().cloned())
            .collect(),
        scopes: vec![name.clone()],
        tags: Vec::new(),
        text: vec![format!(".NET solution {action}")],
    })
    .label(format!(".NET solution {action} `{name}`"))
    .description(format!(
        "Run `dotnet {action}` for {}",
        solution.path.display()
    ))
    .emit(output);
}

fn emit_project_action(
    context: &ScanCtx<'_>,
    project: &DotnetProject,
    action: &str,
    selection: SelectionPolicy,
    output: &mut Detection,
) {
    let directory = context.roots.scan_root.clone();
    let args = if action == "run" {
        vec![
            OsString::from("run"),
            OsString::from("--project"),
            project.path.as_os_str().to_owned(),
        ]
    } else {
        vec![OsString::from(action), project.path.as_os_str().to_owned()]
    };
    CandidateBuilder::tool_default(
        DOTNET_SOURCE,
        context.invocation.intent,
        directory.clone(),
        action,
    )
    .action_key(format!(
        "dotnet:project:{}:{action}",
        project.path.to_string_lossy().replace(['/', '\\'], ":")
    ))
    .tool(DOTNET_TOOL)
    .args(args)
    .cwd(directory)
    .passthrough(if action == "run" {
        PassthroughStyle::DoubleDash
    } else {
        PassthroughStyle::Append
    })
    .selection(selection)
    .base_points(if selection == SelectionPolicy::Automatic {
        85
    } else {
        25
    })
    .lifecycle(if action == "run" {
        Lifecycle::LongRunning
    } else {
        Lifecycle::Finite
    })
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: format!(
            "{} declares .NET project `{}`",
            project.path.display(),
            project.name
        ),
        points: 0,
        source: Some(project.path.clone()),
    })
    .search(SearchDocument {
        identities: vec![action.to_owned(), project.name.clone()],
        target_paths: vec![project.path.clone()],
        scopes: vec![project.name.clone()],
        tags: Vec::new(),
        text: vec![format!(".NET project {action}")],
    })
    .label(format!(".NET project {action} `{}`", project.name))
    .description(format!(
        "Run `dotnet {action}` for {}",
        project.path.display()
    ))
    .emit(output);
}

fn quoted_project_paths(contents: &str) -> Vec<PathBuf> {
    contents
        .lines()
        .filter(|line| line.trim_start().starts_with("Project("))
        .flat_map(|line| line.split('"'))
        .filter(|value| {
            Path::new(value)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    PROJECT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
                })
        })
        .map(|value| PathBuf::from(value.replace('\\', "/")))
        .collect()
}

fn xml_project_paths(contents: &str) -> Vec<PathBuf> {
    ["Path=\"", "path=\""]
        .into_iter()
        .flat_map(|needle| attribute_values(contents, needle))
        .filter(|value| {
            Path::new(value)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    PROJECT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
                })
        })
        .map(|value| PathBuf::from(value.replace('\\', "/")))
        .collect()
}

fn attribute_values<'a>(contents: &'a str, needle: &str) -> Vec<&'a str> {
    let mut values = Vec::new();
    let mut remaining = contents;
    while let Some(start) = remaining.find(needle) {
        let after = &remaining[start + needle.len()..];
        let Some(end) = after.find('"') else {
            break;
        };
        values.push(&after[..end]);
        remaining = &after[end + 1..];
    }
    values
}

fn json_solution_paths(contents: &str) -> Vec<PathBuf> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(contents) else {
        return Vec::new();
    };
    document
        .get("solution")
        .and_then(|solution| solution.get("projects"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(|value| PathBuf::from(value.replace('\\', "/")))
        .collect()
}

fn first_tag_value(contents: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = contents.find(&open)? + open.len();
    let tail = &contents[start..];
    let end = tail.find(&close)?;
    Some(tail[..end].trim().to_owned())
}

fn normalize_member(base: &Path, member: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in base.join(member).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() {
                    return PathBuf::new();
                }
            }
            Component::Normal(part) => output.push(part),
            Component::RootDir | Component::Prefix(_) => return PathBuf::new(),
        }
    }
    output
}

fn safe_relative(path: &Path) -> bool {
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
    fn solution_formats_expose_literal_project_paths() {
        let sln = r#"Project("{type}") = "App", "src\App\App.csproj", "{id}""#;
        assert_eq!(
            quoted_project_paths(sln),
            [PathBuf::from("src/App/App.csproj")]
        );
        let slnx = r#"<Solution><Project Path="tests/App.Tests/App.Tests.csproj" /></Solution>"#;
        assert_eq!(
            xml_project_paths(slnx),
            [PathBuf::from("tests/App.Tests/App.Tests.csproj")]
        );
        let slnf = r#"{"solution":{"path":"App.sln","projects":["src\\App\\App.csproj"]}}"#;
        assert_eq!(
            json_solution_paths(slnf),
            [PathBuf::from("src/App/App.csproj")]
        );
    }

    #[test]
    fn member_normalization_stays_relative() {
        assert_eq!(
            normalize_member(Path::new("solutions"), Path::new("../src/App.csproj")),
            PathBuf::from("src/App.csproj")
        );
        assert!(!safe_relative(Path::new("../App.csproj")));
    }
}
