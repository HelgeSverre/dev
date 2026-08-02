use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::intent::{Intent, Target};
use crate::registry::{ARTISAN, ARTISAN_SOURCE};

use super::composer::{composer_projects, has_laravel_evidence, project_scope, ComposerProject};
use super::{Detection, Detector, ScanCtx};

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
    let mut candidate = Candidate::new(
        format!("artisan:{scope}:serve"),
        ARTISAN,
        ARTISAN_SOURCE,
        Intent::Run,
        "serve",
        "php",
        vec![OsString::from("artisan"), OsString::from("serve")],
        project.directory.clone(),
        70,
        SelectionPolicy::Automatic,
    );
    candidate.lifecycle = Lifecycle::LongRunning;
    candidate.label = "Laravel development server".to_owned();
    candidate.description = "Runs the project-local Artisan serve command".to_owned();
    add_artisan_metadata(&mut candidate, project, &scope, "serve");
    candidate
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
    let mut candidate = Candidate::new(
        format!("artisan:{scope}:test{action_suffix}"),
        ARTISAN,
        ARTISAN_SOURCE,
        Intent::Test,
        action_name,
        "php",
        args,
        project.directory.clone(),
        115,
        SelectionPolicy::Automatic,
    );
    candidate.label = bound_target.as_ref().map_or_else(
        || "Laravel test suite".to_owned(),
        |path| format!("Laravel test {}", path.display()),
    );
    candidate.description = "Runs tests through the project-local Artisan test provider".to_owned();
    add_artisan_metadata(&mut candidate, project, &scope, "test");
    if let Some(target) = bound_target {
        candidate.search.identities.extend(
            target
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned()),
        );
        candidate.search.target_paths.push(target.clone());
        candidate.evidence.push(Evidence {
            kind: EvidenceKind::Rule,
            reason: format!("bound Laravel test provider to {}", target.display()),
            points: 20,
            source: Some(target),
        });
    }
    candidate
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

fn add_artisan_metadata(
    candidate: &mut Candidate,
    project: &ComposerProject,
    scope: &str,
    action: &str,
) {
    let artisan_path = project.manifest_path.with_file_name("artisan");
    candidate.evidence.extend([
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
    ]);
    candidate.search = SearchDocument {
        identities: vec![action.to_owned()],
        target_paths: vec![artisan_path],
        scopes: vec![scope.to_owned(), "laravel".to_owned()],
        tags: vec!["laravel".to_owned(), "php".to_owned(), "artisan".to_owned()],
        text: vec![candidate.description.clone()],
    };
}
