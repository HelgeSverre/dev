use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, CommandLayer, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{MAKE, MAKE_SOURCE};
use crate::scan::IndexedFileType;

use super::{Detection, Detector, ScanCtx};

const MAKEFILES: &[&str] = &["GNUmakefile", "makefile", "Makefile"];

pub struct MakeDetector;

#[derive(Clone, Debug, Eq, PartialEq)]
struct MakeTarget {
    name: String,
    help: Option<String>,
}

impl Detector for MakeDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut output = Detection::default();
        for (manifest_path, relative_manifest, directory) in makefiles(context) {
            let contents = match context.index.manifests.read(&manifest_path) {
                Ok(contents) => contents,
                Err(error) => {
                    output.diagnostics.push(Diagnostic::warning(
                        MAKE,
                        error.to_string(),
                        Some(manifest_path),
                    ));
                    continue;
                }
            };
            let targets = parse_targets(&contents);
            output.candidates.extend(targets.iter().map(|target| {
                target_candidate(
                    context.invocation.intent,
                    &directory,
                    &relative_manifest,
                    target,
                )
            }));
        }
        output
    }
}

fn makefiles(context: &ScanCtx<'_>) -> Vec<(PathBuf, PathBuf, PathBuf)> {
    let mut by_directory = BTreeMap::<PathBuf, (usize, PathBuf)>::new();
    for entry in context.index.all_entries().filter(|entry| {
        entry.file_type == IndexedFileType::File
            && entry
                .relative_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| MAKEFILES.contains(&name))
    }) {
        let Some(filename) = entry
            .relative_path
            .file_name()
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        let priority = MAKEFILES
            .iter()
            .position(|candidate| *candidate == filename)
            .unwrap_or(usize::MAX);
        let relative_directory = entry
            .relative_path
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let absolute = context.roots.scan_root.join(&entry.relative_path);
        by_directory
            .entry(relative_directory)
            .and_modify(|current| {
                if priority < current.0 {
                    *current = (priority, absolute.clone());
                }
            })
            .or_insert((priority, absolute));
    }
    by_directory
        .into_iter()
        .map(|(relative, (_, manifest))| {
            let directory = if relative.as_os_str().is_empty() {
                context.roots.scan_root.clone()
            } else {
                context.roots.scan_root.join(relative)
            };
            let relative_manifest = manifest
                .strip_prefix(&context.roots.scan_root)
                .unwrap_or(&manifest)
                .to_path_buf();
            (manifest, relative_manifest, directory)
        })
        .collect()
}

fn target_candidate(
    intent: Intent,
    directory: &Path,
    relative_manifest: &Path,
    target: &MakeTarget,
) -> Candidate {
    let (selection, base_points, convention) = target_policy(intent, &target.name);
    let scope = directory.file_name().map_or_else(
        || "make-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mut candidate = Candidate::new(
        format!(
            "make:{}:{}",
            normalized_parent(relative_manifest),
            target.name
        ),
        MAKE,
        MAKE_SOURCE,
        intent,
        &target.name,
        "make",
        vec![OsString::from(&target.name)],
        directory.to_path_buf(),
        base_points,
        selection,
    );
    candidate.lifecycle =
        if intent == Intent::Run && matches!(target.name.as_str(), "dev" | "serve" | "start") {
            Lifecycle::LongRunning
        } else {
            Lifecycle::Finite
        };
    candidate.layer = CommandLayer::ProjectFacade;
    candidate.label = format!("Make target {}", target.name);
    candidate.description = target
        .help
        .clone()
        .unwrap_or_else(|| format!("Literal target from {}", relative_manifest.display()));
    candidate.evidence.extend([
        Evidence {
            kind: EvidenceKind::Manifest,
            reason: format!(
                "{} declares literal target `{}`",
                relative_manifest.display(),
                target.name
            ),
            points: 0,
            source: Some(relative_manifest.to_path_buf()),
        },
        Evidence {
            kind: EvidenceKind::Convention,
            reason: convention,
            points: 0,
            source: Some(relative_manifest.to_path_buf()),
        },
    ]);
    candidate.search = SearchDocument {
        identities: vec![target.name.clone()],
        target_paths: vec![relative_manifest.to_path_buf()],
        scopes: vec![scope],
        tags: vec!["make".to_owned(), "makefile".to_owned()],
        text: vec![candidate.description.clone()],
    };
    candidate
}

fn normalized_parent(path: &Path) -> String {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || "root".to_owned(),
            |parent| parent.to_string_lossy().replace(['/', '\\'], ":"),
        )
}

fn target_policy(intent: Intent, name: &str) -> (SelectionPolicy, i32, String) {
    let canonical = match intent {
        Intent::Run => matches!(name, "run" | "dev" | "serve" | "start"),
        Intent::Build => matches!(name, "build" | "all"),
        Intent::Test => matches!(name, "test" | "check"),
    };
    if canonical {
        let points = match name {
            "dev" | "build" | "test" => 90,
            "run" => 85,
            _ => 80,
        };
        return (
            SelectionPolicy::Automatic,
            points,
            format!("`{name}` is a conventional {intent} target"),
        );
    }
    (
        SelectionPolicy::ExplicitHint,
        15,
        "non-canonical literal Make target requires an identity hint".to_owned(),
    )
}

fn parse_targets(contents: &str) -> Vec<MakeTarget> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    let mut preceding_help = None;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(help) = trimmed.strip_prefix("##") {
            preceding_help = nonempty(help.trim());
            continue;
        }
        if trimmed.is_empty() {
            preceding_help = None;
            continue;
        }
        if line.starts_with('\t') || trimmed.starts_with('#') {
            preceding_help = None;
            continue;
        }
        let (rule, inline_help) = line
            .split_once("##")
            .map_or((line, None), |(rule, help)| (rule, nonempty(help.trim())));
        let rule = rule.split('#').next().unwrap_or_default();
        let Some((left, right)) = rule.split_once(':') else {
            preceding_help = None;
            continue;
        };
        if left.contains('=') || left.contains('$') || right.trim_start().starts_with('=') {
            preceding_help = None;
            continue;
        }
        let help = inline_help.or_else(|| preceding_help.take());
        for name in left
            .split_ascii_whitespace()
            .filter(|name| literal_target(name))
        {
            if seen.insert(name.to_owned()) {
                targets.push(MakeTarget {
                    name: name.to_owned(),
                    help: help.clone(),
                });
            }
        }
        preceding_help = None;
    }
    targets
}

fn literal_target(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['%', '$', '*', '?', '\\', '=', '&'])
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '.' | '/' | '@' | '+')
        })
}

fn nonempty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_scanner_keeps_literals_and_help_only() {
        let targets = parse_targets(
            "# comment\n## Start locally\ndev: ## inline wins\n\t@echo ok\n.PHONY: dev\n%.o: %.c\n$(NAME):\nfoo bar: dep\nVAR := value\n",
        );
        assert_eq!(
            targets,
            [
                MakeTarget {
                    name: "dev".to_owned(),
                    help: Some("inline wins".to_owned()),
                },
                MakeTarget {
                    name: "foo".to_owned(),
                    help: None,
                },
                MakeTarget {
                    name: "bar".to_owned(),
                    help: None,
                },
            ]
        );
    }
}
