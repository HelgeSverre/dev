use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Evidence, EvidenceKind, Lifecycle, PassthroughStyle, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{ScanContribution, WorkspaceContributor, GRADLE, GRADLE_SOURCE, GRADLE_TOOL};
use crate::scan::{DiscoveryFiles, IndexedFileType};

use super::wrapper::{locally_usable_wrapper, WrapperKind};
use super::{CandidateBuilder, Detection, Detector, ScanCtx};

const GRADLE_FILES: &[&str] = &[
    "settings.gradle",
    "settings.gradle.kts",
    "build.gradle",
    "build.gradle.kts",
];

pub struct GradleDetector;
pub struct GradleWorkspaceContributor;

impl WorkspaceContributor for GradleWorkspaceContributor {
    fn scan_contribution(&self, root: &Path, files: &DiscoveryFiles) -> ScanContribution {
        let settings = [
            root.join("settings.gradle"),
            root.join("settings.gradle.kts"),
        ]
        .into_iter()
        .find_map(|path| files.read(&path).ok())
        .unwrap_or_default();
        let mut includes = literal_project_includes(&settings)
            .into_iter()
            .flat_map(|project| {
                let directory = project.trim_start_matches(':').replace(':', "/");
                [
                    format!("{directory}/build.gradle"),
                    format!("{directory}/build.gradle.kts"),
                ]
            })
            .collect::<Vec<_>>();
        includes.sort();
        includes.dedup();
        ScanContribution {
            includes,
            excludes: Vec::new(),
        }
    }
}

#[derive(Default)]
struct GradleProject {
    files: Vec<PathBuf>,
    tasks: BTreeSet<String>,
    application: bool,
}

impl Detector for GradleDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut projects = BTreeMap::<PathBuf, GradleProject>::new();
        let mut output = Detection::default();
        for entry in context.index.all_entries() {
            if entry.file_type != IndexedFileType::File
                || !entry
                    .relative_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| GRADLE_FILES.contains(&name))
            {
                continue;
            }
            let absolute = context.roots.scan_root.join(&entry.relative_path);
            let directory = absolute
                .parent()
                .unwrap_or(&context.roots.scan_root)
                .to_path_buf();
            let contents = match context.index.manifests.read(&absolute) {
                Ok(contents) => contents,
                Err(error) => {
                    output.diagnostics.push(Diagnostic::warning(
                        GRADLE,
                        error.to_string(),
                        Some(absolute),
                    ));
                    continue;
                }
            };
            let project = projects.entry(directory).or_default();
            project.files.push(entry.relative_path.clone());
            project.tasks.extend(literal_tasks(&contents));
            project.application |= has_application_plugin(&contents);
        }
        for (directory, project) in projects {
            if is_generated_flutter_android_project(context, &directory) {
                continue;
            }
            emit_project(context, &directory, project, &mut output);
        }
        output
    }
}

fn is_generated_flutter_android_project(context: &ScanCtx<'_>, directory: &Path) -> bool {
    if context
        .invocation
        .target
        .anchor_directory()
        .starts_with(directory)
    {
        return false;
    }

    directory
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(&context.roots.scan_root))
        .any(|ancestor| {
            let is_android_subtree = directory
                .strip_prefix(ancestor)
                .ok()
                .and_then(|relative| relative.components().next())
                .is_some_and(|component| component.as_os_str() == "android");
            if !is_android_subtree {
                return false;
            }
            context
                .index
                .manifests
                .read(&ancestor.join("pubspec.yaml"))
                .is_ok_and(|contents| super::dart::pubspec_declares_flutter(&contents))
        })
}

fn emit_project(
    context: &ScanCtx<'_>,
    directory: &Path,
    mut project: GradleProject,
    output: &mut Detection,
) {
    project.files.sort();
    project.files.dedup();
    let mut tasks = match context.invocation.intent {
        Intent::Build => BTreeSet::from(["build".to_owned()]),
        Intent::Test => BTreeSet::from(["test".to_owned()]),
        Intent::Run => BTreeSet::new(),
    };
    if context.invocation.intent == Intent::Run && project.application {
        tasks.insert("run".to_owned());
    }
    tasks.extend(
        project
            .tasks
            .into_iter()
            .filter(|task| match context.invocation.intent {
                Intent::Run => true,
                Intent::Build => matches!(task.as_str(), "build" | "assemble" | "check"),
                Intent::Test => matches!(task.as_str(), "test" | "check"),
            }),
    );

    let wrapper = locally_usable_wrapper(directory, WrapperKind::Gradle, &context.index.manifests);
    let relative_directory = directory
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(directory);
    let scope = directory.file_name().map_or_else(
        || "gradle-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    for task in tasks {
        let (selection, points) =
            task_policy(context.invocation.intent, &task, project.application);
        let description = if wrapper.is_some() {
            "Gradle task using an already-cached project wrapper"
        } else {
            "Gradle task using the global Gradle installation"
        };
        let mut builder = CandidateBuilder::tool_default(
            GRADLE_SOURCE,
            context.invocation.intent,
            directory.to_path_buf(),
            &task,
        )
        .action_key(format!(
            "gradle:{}:{}",
            relative_directory
                .to_string_lossy()
                .replace(['/', '\\'], ":"),
            task
        ));
        builder = if let Some(wrapper) = &wrapper {
            builder.program_path(wrapper.as_os_str().to_owned())
        } else {
            builder.tool(GRADLE_TOOL)
        };
        builder
            .args([OsString::from(&task)])
            .cwd(directory.to_path_buf())
            .passthrough(PassthroughStyle::Append)
            .selection(selection)
            .base_points(points)
            .lifecycle(
                if context.invocation.intent == Intent::Run
                    && matches!(task.as_str(), "run" | "bootRun" | "dev")
                {
                    Lifecycle::LongRunning
                } else {
                    Lifecycle::Finite
                },
            )
            .evidence(Evidence {
                kind: EvidenceKind::Manifest,
                reason: format!(
                    "{} declares a Gradle project{}",
                    project.files.first().map_or_else(
                        || "Gradle files".to_owned(),
                        |path| path.display().to_string()
                    ),
                    if project.application {
                        " with the application plugin"
                    } else {
                        ""
                    }
                ),
                points: 0,
                source: project.files.first().cloned(),
            })
            .search(SearchDocument {
                identities: vec![task.clone()],
                target_paths: project.files.clone(),
                scopes: vec![scope.clone()],
                tags: Vec::new(),
                text: vec![description.to_owned()],
            })
            .label(format!("Gradle task `{task}`"))
            .description(description)
            .emit(output);
    }
}

fn task_policy(intent: Intent, task: &str, application: bool) -> (SelectionPolicy, i32) {
    let canonical = match intent {
        Intent::Run => matches!(task, "run" | "bootRun" | "dev") && (application || task != "run"),
        Intent::Build => matches!(task, "build" | "assemble"),
        Intent::Test => matches!(task, "test" | "check"),
    };
    if canonical {
        (
            SelectionPolicy::Automatic,
            if task == "build" || task == "test" || task == "run" {
                90
            } else {
                75
            },
        )
    } else {
        (SelectionPolicy::ExplicitHint, 15)
    }
}

fn has_application_plugin(contents: &str) -> bool {
    let mut in_plugins_block = false;
    for line in contents.lines().map(strip_comment) {
        let compact = line.split_ascii_whitespace().collect::<String>();
        let inline_application = compact
            .strip_prefix("plugins{")
            .and_then(|plugins| plugins.strip_suffix('}'))
            .is_some_and(|plugins| plugins.split(';').any(|plugin| plugin == "application"));
        if compact.contains("id(\"application\")")
            || compact.contains("id('application')")
            || compact.contains("id'application'")
            || compact.contains("plugin:'application'")
            || inline_application
            || (in_plugins_block && compact == "application")
        {
            return true;
        }
        if compact.starts_with("plugins{") {
            in_plugins_block = !compact.ends_with('}');
        } else if in_plugins_block && compact.contains('}') {
            in_plugins_block = false;
        }
    }
    false
}

fn literal_tasks(contents: &str) -> BTreeSet<String> {
    let mut tasks = BTreeSet::new();
    for line in contents.lines().map(strip_comment).map(str::trim) {
        for prefix in ["task ", "tasks.register(", "tasks.create(", "tasks.named("] {
            let Some(rest) = line.strip_prefix(prefix) else {
                continue;
            };
            let candidate = if prefix == "task " {
                rest.split(|character: char| {
                    character.is_ascii_whitespace() || matches!(character, '(' | '{' | ':')
                })
                .next()
                .unwrap_or_default()
            } else {
                quoted_prefix(rest).unwrap_or_default()
            };
            if valid_task(candidate) {
                tasks.insert(candidate.to_owned());
            }
        }
        if let Some((name, rest)) = line.split_once(" by tasks.registering") {
            let name = name.trim().strip_prefix("val ").unwrap_or(name.trim());
            if rest.trim().is_empty() && valid_task(name) {
                tasks.insert(name.to_owned());
            }
        }
    }
    tasks
}

fn literal_project_includes(contents: &str) -> BTreeSet<String> {
    let mut projects = BTreeSet::new();
    for line in contents.lines().map(strip_comment).map(str::trim) {
        let Some(rest) = line.strip_prefix("include") else {
            continue;
        };
        if !rest.starts_with([' ', '(']) {
            continue;
        }
        let mut remaining = rest;
        while let Some(start) = remaining.find(['\'', '"']) {
            let quote = remaining.as_bytes()[start] as char;
            let tail = &remaining[start + 1..];
            let Some(end) = tail.find(quote) else {
                break;
            };
            let project = &tail[..end];
            if project.starts_with(':') && project.split(':').skip(1).all(valid_task) {
                projects.insert(project.to_owned());
            }
            remaining = &tail[end + 1..];
        }
    }
    projects
}

fn quoted_prefix(value: &str) -> Option<&str> {
    let quote = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))?;
    let tail = &value[quote.len_utf8()..];
    let end = tail.find(quote)?;
    Some(&tail[..end])
}

fn valid_task(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | ':')
        })
}

fn strip_comment(line: &str) -> &str {
    line.split_once("//").map_or(line, |(before, _)| before)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_gradle_task_forms_and_application_plugin_are_recognized() {
        let source = r#"
            plugins { id("application") }
            task deploy {
            tasks.register("verify")
            tasks.create('bundle')
            val smoke by tasks.registering
            tasks.register(dynamicName)
        "#;
        assert!(has_application_plugin(source));
        assert!(has_application_plugin("plugins { application }"));
        assert!(has_application_plugin("plugins { java; application }"));
        assert!(has_application_plugin("plugins {\n  application\n}"));
        assert!(has_application_plugin("plugins {\n  id 'application'\n}"));
        assert_eq!(
            literal_tasks(source),
            BTreeSet::from([
                "bundle".to_owned(),
                "deploy".to_owned(),
                "smoke".to_owned(),
                "verify".to_owned(),
            ])
        );
    }

    #[test]
    fn static_settings_includes_become_member_projects() {
        assert_eq!(
            literal_project_includes("include(\":app\", ':libs:core')\ninclude dynamicName\n"),
            BTreeSet::from([":app".to_owned(), ":libs:core".to_owned()])
        );
    }

    #[test]
    fn flutter_pubspec_recognition_is_limited_to_sdk_dependencies() {
        assert!(super::super::dart::pubspec_declares_flutter(
            "dependencies:\n  flutter:\n    sdk: flutter\n"
        ));
        assert!(!super::super::dart::pubspec_declares_flutter(
            "dependencies:\n  flutter: ^1.0.0\n"
        ));
    }
}
