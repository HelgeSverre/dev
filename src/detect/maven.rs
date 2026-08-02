use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::candidate::{
    Evidence, EvidenceKind, Lifecycle, PassthroughStyle, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{
    RootClassification, ScanContribution, WorkspaceContributor, MAVEN, MAVEN_SOURCE, MAVEN_TOOL,
};
use crate::scan::{DiscoveryFiles, IndexedFileType};

use super::wrapper::{locally_usable_wrapper, WrapperKind};
use super::{CandidateBuilder, Detection, Detector, ScanCtx};

pub struct MavenDetector;
pub struct MavenWorkspaceContributor;

impl WorkspaceContributor for MavenWorkspaceContributor {
    fn classify_root(&self, marker: &Path, files: &DiscoveryFiles) -> RootClassification {
        let Ok(contents) = files.read(marker) else {
            return RootClassification::Neither;
        };
        if !contents.contains("<project") {
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
        let contents = files.read(&root.join("pom.xml")).unwrap_or_default();
        let mut includes = section_tag_values(&contents, "modules", "module")
            .into_iter()
            .filter(|module| safe_module(module))
            .map(|module| format!("{}/pom.xml", module.trim_end_matches(['/', '\\'])))
            .collect::<Vec<_>>();
        includes.sort();
        includes.dedup();
        ScanContribution {
            includes,
            excludes: Vec::new(),
        }
    }
}

fn safe_module(module: &str) -> bool {
    let path = Path::new(module);
    !module.is_empty()
        && !path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        && !module.contains(['$', '{', '}', '*', '?'])
}

#[derive(Clone, Debug)]
struct MavenProject {
    manifest: PathBuf,
    directory: PathBuf,
    artifact: Option<String>,
    packaging: Option<String>,
    modules: Vec<String>,
    plugins: Vec<String>,
}

impl Detector for MavenDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut manifests = context
            .index
            .all_entries()
            .filter(|entry| {
                entry.file_type == IndexedFileType::File
                    && entry
                        .relative_path
                        .file_name()
                        .is_some_and(|name| name == "pom.xml")
            })
            .map(|entry| entry.relative_path.clone())
            .collect::<Vec<_>>();
        manifests.sort();
        manifests.dedup();

        let mut output = Detection::default();
        for manifest in manifests {
            let absolute = context.roots.scan_root.join(&manifest);
            let contents = match context.index.manifests.read(&absolute) {
                Ok(contents) => contents,
                Err(error) => {
                    output.diagnostics.push(Diagnostic::warning(
                        MAVEN,
                        error.to_string(),
                        Some(absolute),
                    ));
                    continue;
                }
            };
            if !contents.contains("<project") {
                output.diagnostics.push(Diagnostic::warning(
                    MAVEN,
                    "pom.xml has no literal `<project>` root",
                    Some(absolute),
                ));
                continue;
            }
            let directory = absolute
                .parent()
                .unwrap_or(&context.roots.scan_root)
                .to_path_buf();
            let project = MavenProject {
                manifest,
                directory,
                artifact: tag_values(&contents, "artifactId").into_iter().next(),
                packaging: tag_values(&contents, "packaging").into_iter().next(),
                modules: section_tag_values(&contents, "modules", "module"),
                plugins: section_tag_values(&contents, "plugins", "artifactId"),
            };
            emit_project(context, &project, &mut output);
        }
        output
    }
}

fn emit_project(context: &ScanCtx<'_>, project: &MavenProject, output: &mut Detection) {
    let goals = match context.invocation.intent {
        Intent::Build => vec![("package", 90, SelectionPolicy::Automatic)],
        Intent::Test => vec![("test", 90, SelectionPolicy::Automatic)],
        Intent::Run => run_goals(&project.plugins),
    };
    let wrapper = locally_usable_wrapper(
        &project.directory,
        WrapperKind::Maven,
        &context.index.manifests,
    );
    let name = project.artifact.as_deref().unwrap_or("maven-project");
    let scope = project.directory.file_name().map_or_else(
        || name.to_owned(),
        |directory| directory.to_string_lossy().into_owned(),
    );
    for (goal, points, selection) in goals {
        let description = match project.packaging.as_deref() {
            Some(packaging) => format!("Maven `{goal}` for `{name}` ({packaging} packaging)"),
            None => format!("Maven `{goal}` for `{name}`"),
        };
        let mut builder = CandidateBuilder::tool_default(
            MAVEN_SOURCE,
            context.invocation.intent,
            project.directory.clone(),
            goal,
        )
        .action_key(format!(
            "maven:{}:{goal}",
            project.manifest.to_string_lossy().replace(['/', '\\'], ":")
        ));
        builder = if let Some(wrapper) = &wrapper {
            builder.program_path(wrapper.as_os_str().to_owned())
        } else {
            builder.tool(MAVEN_TOOL)
        };
        let mut evidence = vec![Evidence {
            kind: EvidenceKind::Manifest,
            reason: format!(
                "{} declares Maven project `{name}`",
                project.manifest.display()
            ),
            points: 0,
            source: Some(project.manifest.clone()),
        }];
        if !project.modules.is_empty() {
            evidence.push(Evidence {
                kind: EvidenceKind::Manifest,
                reason: format!(
                    "pom.xml declares {} literal reactor module(s)",
                    project.modules.len()
                ),
                points: 0,
                source: Some(project.manifest.clone()),
            });
        }
        builder
            .args([OsString::from(goal)])
            .cwd(project.directory.clone())
            .passthrough(PassthroughStyle::Append)
            .selection(selection)
            .base_points(points)
            .lifecycle(if context.invocation.intent == Intent::Run {
                Lifecycle::LongRunning
            } else {
                Lifecycle::Finite
            })
            .evidence_all(evidence)
            .search(SearchDocument {
                identities: vec![goal.to_owned(), name.to_owned()],
                target_paths: vec![project.manifest.clone()],
                scopes: vec![scope.clone()],
                tags: Vec::new(),
                text: vec![description.clone()],
            })
            .label(format!("Maven goal `{goal}`"))
            .description(description)
            .emit(output);
    }
}

fn run_goals(plugins: &[String]) -> Vec<(&'static str, i32, SelectionPolicy)> {
    let mut goals = Vec::new();
    if plugins
        .iter()
        .any(|plugin| plugin == "spring-boot-maven-plugin")
    {
        goals.push(("spring-boot:run", 90, SelectionPolicy::Automatic));
    }
    if plugins
        .iter()
        .any(|plugin| plugin == "quarkus-maven-plugin")
    {
        goals.push(("quarkus:dev", 85, SelectionPolicy::Automatic));
    }
    if plugins.iter().any(|plugin| plugin == "exec-maven-plugin") {
        goals.push(("exec:java", 75, SelectionPolicy::Automatic));
    }
    goals
}

fn section_tag_values(contents: &str, section: &str, tag: &str) -> Vec<String> {
    sections(contents, section)
        .into_iter()
        .flat_map(|section| tag_values(section, tag))
        .collect()
}

fn sections<'a>(contents: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut sections = Vec::new();
    let mut remaining = contents;
    while let Some(start) = remaining.find(&open) {
        let after = &remaining[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        sections.push(&after[..end]);
        remaining = &after[end + close.len()..];
    }
    sections
}

fn tag_values(contents: &str, tag: &str) -> Vec<String> {
    sections(contents, tag)
        .into_iter()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('<'))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_pom_data_extracts_modules_and_known_plugins() {
        let pom = r#"
          <project>
            <artifactId>app</artifactId>
            <packaging>jar</packaging>
            <modules><module>core</module><module>web</module></modules>
            <build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build>
          </project>
        "#;
        assert_eq!(tag_values(pom, "artifactId")[0], "app");
        assert_eq!(
            section_tag_values(pom, "modules", "module"),
            ["core", "web"]
        );
        assert_eq!(
            run_goals(&section_tag_values(pom, "plugins", "artifactId"))[0].0,
            "spring-boot:run"
        );
    }
}
