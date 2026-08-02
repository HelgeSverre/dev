use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::intent::{Intent, Target};
use crate::registry::{ARTISAN_SOURCE, PHP_TOOL};

use super::composer::{composer_projects, has_laravel_evidence, project_scope, ComposerProject};
use super::{CandidateBuilder, Detection, Detector, ScanCtx};

pub struct ArtisanDetector;

impl Detector for ArtisanDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let (projects, _) = composer_projects(context, false);
        let mut output = Detection::default();
        for project in projects.into_iter().filter(|project| {
            has_laravel_evidence(&project.manifest) && project.directory.join("artisan").is_file()
        }) {
            let candidate = match context.invocation.intent {
                Intent::Run => Some(serve_candidate(&project)),
                Intent::Test => Some(test_candidate(context, &project)),
                Intent::Build => None,
            };
            output.candidates.extend(candidate);
        }
        output
    }
}

fn serve_candidate(project: &ComposerProject) -> Candidate {
    let scope = project_scope(project);
    let description = "Runs the project-local Artisan serve command".to_owned();
    let (evidence, search) = artisan_metadata(project, &scope, "serve", &description, None);
    CandidateBuilder::tool_default(
        ARTISAN_SOURCE,
        Intent::Run,
        project.directory.clone(),
        "serve",
    )
    .action_key(format!("artisan:{scope}:serve"))
    .tool(PHP_TOOL)
    .args([OsString::from("artisan"), OsString::from("serve")])
    .cwd(project.directory.clone())
    .selection(SelectionPolicy::Automatic)
    .base_points(70)
    .lifecycle(Lifecycle::LongRunning)
    .label("Laravel development server")
    .description(description)
    .evidence_all(evidence)
    .search(search)
    .build()
    .expect("Artisan serve candidate registration is valid")
}

fn test_candidate(context: &ScanCtx<'_>, project: &ComposerProject) -> Candidate {
    let scope = project_scope(project);
    let bound_target = explicit_php_test_target(context, project);
    let mut args = vec![OsString::from("artisan"), OsString::from("test")];
    if let Some(path) = &bound_target {
        args.push(path.as_os_str().to_owned());
    }
    let action_suffix = bound_target.as_ref().map_or_else(String::new, |path| {
        format!(
            ":target:{}",
            path.to_string_lossy().replace(['/', '\\'], ":")
        )
    });
    let action_name = bound_target
        .as_deref()
        .and_then(Path::file_stem)
        .map_or_else(
            || "test".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
    let label = bound_target.as_ref().map_or_else(
        || "Laravel test suite".to_owned(),
        |path| format!("Laravel test {}", path.display()),
    );
    let description = "Runs tests through the project-local Artisan test provider".to_owned();
    let (evidence, search) = artisan_metadata(
        project,
        &scope,
        "test",
        &description,
        bound_target.as_deref(),
    );
    CandidateBuilder::tool_default(
        ARTISAN_SOURCE,
        Intent::Test,
        project.directory.clone(),
        action_name,
    )
    .action_key(format!("artisan:{scope}:test{action_suffix}"))
    .tool(PHP_TOOL)
    .args(args)
    .cwd(project.directory.clone())
    .selection(SelectionPolicy::Automatic)
    .base_points(115)
    .label(label)
    .description(description)
    .evidence_all(evidence)
    .search(search)
    .build()
    .expect("Artisan test candidate registration is valid")
}

fn explicit_php_test_target(context: &ScanCtx<'_>, project: &ComposerProject) -> Option<PathBuf> {
    let Target::File(target) = &context.invocation.target else {
        return None;
    };
    (target
        .extension()
        .is_some_and(|extension| extension == "php")
        && target.starts_with(project.directory.join("tests")))
    .then(|| {
        target
            .strip_prefix(&project.directory)
            .unwrap_or(target)
            .to_path_buf()
    })
}

fn artisan_metadata(
    project: &ComposerProject,
    scope: &str,
    action: &str,
    description: &str,
    target: Option<&Path>,
) -> (Vec<Evidence>, SearchDocument) {
    let artisan_path = project.manifest_path.with_file_name("artisan");
    let mut evidence = vec![
        Evidence {
            kind: EvidenceKind::Manifest,
            reason: "composer.json declares Laravel framework support".to_owned(),
            points: 0,
            source: Some(project.manifest_path.clone()),
        },
        Evidence {
            kind: EvidenceKind::Convention,
            reason: format!("project contains Artisan entrypoint for {action}"),
            points: 0,
            source: Some(artisan_path.clone()),
        },
    ];
    if let Some(target) = target {
        evidence.push(Evidence {
            kind: EvidenceKind::Rule,
            reason: format!("bound Laravel test provider to {}", target.display()),
            points: 20,
            source: Some(target.to_path_buf()),
        });
    }
    let mut identities = vec![action.to_owned()];
    identities.extend(
        target
            .and_then(Path::file_stem)
            .map(|name| name.to_string_lossy().into_owned()),
    );
    let mut target_paths = vec![artisan_path];
    target_paths.extend(target.map(Path::to_path_buf));
    let search = SearchDocument {
        identities,
        target_paths,
        scopes: vec![scope.to_owned(), "laravel".to_owned()],
        tags: vec!["laravel".to_owned(), "php".to_owned(), "artisan".to_owned()],
        text: vec![description.to_owned()],
    };
    (evidence, search)
}
