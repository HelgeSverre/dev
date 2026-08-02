use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{Candidate, Evidence, EvidenceKind, SearchDocument, SelectionPolicy};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{GO, GO_SOURCE};
use crate::scan::IndexedFileType;

use super::{Detection, Detector, ScanCtx};

pub struct GoDetector;

#[derive(Clone, Debug)]
struct GoModule {
    manifest_path: PathBuf,
    relative_directory: PathBuf,
    directory: PathBuf,
    module_path: String,
}

impl Detector for GoDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let (mut modules, mut diagnostics) = modules(context);
        modules.sort_by(|left, right| {
            right
                .relative_directory
                .components()
                .count()
                .cmp(&left.relative_directory.components().count())
                .then_with(|| left.manifest_path.cmp(&right.manifest_path))
        });
        let (packages, source_diagnostics) = package_directories(context);
        diagnostics.extend(source_diagnostics);
        let mut main_packages = vec![BTreeMap::<PathBuf, PathBuf>::new(); modules.len()];
        for (directory, (package, source)) in &packages {
            if package != "main" {
                continue;
            }
            if let Some((index, module)) = modules
                .iter()
                .enumerate()
                .find(|(_, module)| directory.starts_with(&module.relative_directory))
            {
                let relative = directory
                    .strip_prefix(&module.relative_directory)
                    .unwrap_or(directory)
                    .to_path_buf();
                main_packages[index].insert(relative, source.clone());
            }
        }

        let mut output = Detection {
            candidates: Vec::new(),
            diagnostics,
        };
        for (index, module) in modules.iter().enumerate() {
            match context.invocation.intent {
                Intent::Run | Intent::Build => {
                    output.candidates.extend(main_packages[index].iter().map(
                        |(package_directory, source)| {
                            main_candidate(
                                context.invocation.intent,
                                module,
                                package_directory,
                                source,
                            )
                        },
                    ));
                }
                Intent::Test => output
                    .candidates
                    .extend(test_candidates(context, module, &modules, &packages)),
            }
        }
        output
    }
}

fn modules(context: &ScanCtx<'_>) -> (Vec<GoModule>, Vec<Diagnostic>) {
    let mut paths = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "go.mod")
        })
        .map(|entry| entry.relative_path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();

    let mut modules = Vec::new();
    let mut diagnostics = Vec::new();
    for manifest_path in paths {
        let absolute = context.roots.scan_root.join(&manifest_path);
        let contents = match context.index.manifests.read(&absolute) {
            Ok(contents) => contents,
            Err(error) => {
                diagnostics.push(Diagnostic::warning(GO, error.to_string(), Some(absolute)));
                continue;
            }
        };
        let relative_directory = manifest_path
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let directory = absolute
            .parent()
            .unwrap_or(&context.roots.scan_root)
            .to_path_buf();
        let module_path = parse_module_path(&contents).unwrap_or_else(|| {
            diagnostics.push(Diagnostic::warning(
                GO,
                "go.mod has no static module directive; using the directory as scope",
                Some(absolute.clone()),
            ));
            directory.file_name().map_or_else(
                || ".".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        });
        modules.push(GoModule {
            manifest_path,
            relative_directory,
            directory,
            module_path,
        });
    }
    (modules, diagnostics)
}

fn package_directories(
    context: &ScanCtx<'_>,
) -> (BTreeMap<PathBuf, (String, PathBuf)>, Vec<Diagnostic>) {
    let mut sources = BTreeMap::<PathBuf, BTreeSet<PathBuf>>::new();
    for entry in context.index.all_entries().filter(|entry| {
        entry.file_type == IndexedFileType::File
            && entry
                .relative_path
                .extension()
                .is_some_and(|extension| extension == "go")
            && !entry
                .relative_path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with("_test.go"))
    }) {
        let directory = entry
            .relative_path
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        sources
            .entry(directory)
            .or_default()
            .insert(entry.relative_path.clone());
    }

    let mut packages = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (directory, paths) in sources {
        for path in paths {
            let absolute = context.roots.scan_root.join(&path);
            let contents = match context.index.manifests.read(&absolute) {
                Ok(contents) => contents,
                Err(error) => {
                    diagnostics.push(Diagnostic::warning(GO, error.to_string(), Some(absolute)));
                    continue;
                }
            };
            if let Some(package) = parse_package_clause(&contents) {
                packages.insert(directory.clone(), (package, path));
                break;
            }
        }
    }
    (packages, diagnostics)
}

fn main_candidate(
    intent: Intent,
    module: &GoModule,
    package_directory: &Path,
    source: &Path,
) -> Candidate {
    let package_argument = package_argument(package_directory);
    let identity = package_identity(module, package_directory);
    let action = match intent {
        Intent::Run => "run",
        Intent::Build => "build",
        Intent::Test => unreachable!("test candidates are built separately"),
    };
    let base_points = if package_directory.as_os_str().is_empty() {
        95
    } else {
        80
    };
    let mut candidate = Candidate::new(
        format!(
            "go:{}:{action}:{}",
            module.module_path,
            stable_path_suffix(package_directory)
        ),
        GO,
        GO_SOURCE,
        intent,
        &identity,
        "go",
        vec![OsString::from(action), package_argument],
        module.directory.clone(),
        base_points,
        SelectionPolicy::Automatic,
    );
    candidate.label = format!("Go {action} {identity}");
    candidate.description = format!("Go main package {}", package_display(package_directory));
    candidate.evidence.push(Evidence {
        kind: EvidenceKind::Manifest,
        reason: format!("{} declares package main", source.display()),
        points: 0,
        source: Some(source.to_path_buf()),
    });
    candidate.search = SearchDocument {
        identities: vec![identity, action.to_owned()],
        target_paths: vec![source.to_path_buf(), package_directory.to_path_buf()],
        scopes: vec![module.module_path.clone()],
        tags: vec!["go".to_owned(), "golang".to_owned()],
        text: vec![candidate.description.clone()],
    };
    candidate
}

fn test_candidates(
    context: &ScanCtx<'_>,
    module: &GoModule,
    modules: &[GoModule],
    packages: &BTreeMap<PathBuf, (String, PathBuf)>,
) -> Vec<Candidate> {
    let local_package = context
        .invocation
        .target
        .anchor_directory()
        .strip_prefix(&module.directory)
        .ok()
        .map(Path::to_path_buf)
        .filter(|relative| {
            !relative.as_os_str().is_empty()
                || matches!(context.invocation.target, crate::intent::Target::File(_))
        })
        .filter(|relative| {
            let from_scan_root = module.relative_directory.join(relative);
            packages.contains_key(&from_scan_root) && module_owns(module, &from_scan_root, modules)
        });
    if let Some(local_package) = local_package {
        return vec![test_candidate(module, Some(local_package))];
    }

    let mut candidates = vec![test_candidate(module, None)];
    if context.invocation.hints.is_empty() {
        return candidates;
    }
    let query = crate::query::normalize_query(&context.invocation.hints);
    candidates.extend(
        packages
            .keys()
            .filter(|directory| module_owns(module, directory, modules))
            .filter_map(|directory| {
                directory
                    .strip_prefix(&module.relative_directory)
                    .ok()
                    .map(Path::to_path_buf)
            })
            .map(|directory| test_candidate(module, Some(directory)))
            .filter(|candidate| {
                let matched =
                    crate::query::match_candidate(candidate, &query, context.invocation.chaos);
                matched.highest_class == Some(crate::query::MatchClass::Identity)
                    && matched.matched_meaningful_terms > 0
            }),
    );
    candidates
}

fn module_owns(module: &GoModule, directory: &Path, modules: &[GoModule]) -> bool {
    modules
        .iter()
        .find(|candidate| directory.starts_with(&candidate.relative_directory))
        .is_some_and(|owner| owner.manifest_path == module.manifest_path)
}

fn test_candidate(module: &GoModule, local_package: Option<PathBuf>) -> Candidate {
    let package = local_package
        .as_deref()
        .map_or_else(|| OsString::from("./..."), package_argument);
    let identity = local_package
        .as_deref()
        .map_or_else(|| "all".to_owned(), |path| package_identity(module, path));
    let mut candidate = Candidate::new(
        format!(
            "go:{}:test:{}",
            module.module_path,
            local_package
                .as_deref()
                .map_or_else(|| "all".to_owned(), stable_path_suffix)
        ),
        GO,
        GO_SOURCE,
        Intent::Test,
        &identity,
        "go",
        vec![OsString::from("test"), package],
        module.directory.clone(),
        95,
        SelectionPolicy::Automatic,
    );
    candidate.label = if local_package.is_some() {
        format!("Go package tests {identity}")
    } else {
        format!("Go module tests {}", module.module_path)
    };
    candidate.description = "Runs Go tests through the selected module".to_owned();
    candidate.evidence.push(Evidence {
        kind: EvidenceKind::Manifest,
        reason: "go.mod defines the selected test module".to_owned(),
        points: 0,
        source: Some(module.manifest_path.clone()),
    });
    if let Some(target) = &local_package {
        candidate.evidence.push(Evidence {
            kind: EvidenceKind::Rule,
            reason: format!("bound Go test provider to package {}", target.display()),
            points: 20,
            source: Some(target.clone()),
        });
    }
    candidate.search = SearchDocument {
        identities: vec![identity, "test".to_owned()],
        target_paths: local_package
            .into_iter()
            .chain(std::iter::once(module.manifest_path.clone()))
            .collect(),
        scopes: vec![module.module_path.clone()],
        tags: vec!["go".to_owned(), "golang".to_owned()],
        text: vec![candidate.description.clone()],
    };
    candidate
}

fn parse_module_path(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let line = line
            .split_once("//")
            .map_or(line, |(before, _)| before)
            .trim();
        line.strip_prefix("module")
            .filter(|rest| rest.starts_with(char::is_whitespace))
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(|path| path.trim_matches('"').to_owned())
    })
}

fn parse_package_clause(contents: &str) -> Option<String> {
    let mut rest = contents.trim_start_matches('\u{feff}');
    loop {
        rest = rest.trim_start();
        if let Some(comment) = rest.strip_prefix("//") {
            rest = comment.split_once('\n')?.1;
            continue;
        }
        if let Some(comment) = rest.strip_prefix("/*") {
            rest = comment.split_once("*/")?.1;
            continue;
        }
        break;
    }
    let rest = rest.strip_prefix("package")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let name = rest.split_whitespace().next()?;
    name.chars()
        .all(|character| character == '_' || character.is_alphanumeric())
        .then(|| name.to_owned())
}

fn package_argument(directory: &Path) -> OsString {
    if directory.as_os_str().is_empty() {
        OsString::from(".")
    } else {
        PathBuf::from(".").join(directory).into_os_string()
    }
}

fn package_identity(module: &GoModule, directory: &Path) -> String {
    directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .or_else(|| module.module_path.rsplit('/').next().map(str::to_owned))
        .unwrap_or_else(|| "main".to_owned())
}

fn package_display(directory: &Path) -> String {
    if directory.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        PathBuf::from(".")
            .join(directory)
            .to_string_lossy()
            .into_owned()
    }
}

fn stable_path_suffix(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "root".to_owned()
    } else {
        path.to_string_lossy().replace(['/', '\\'], ":")
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_module_path, parse_package_clause};

    #[test]
    fn static_go_headers_ignore_comments() {
        assert_eq!(
            parse_module_path("// comment\nmodule example.com/acme/tool\n"),
            Some("example.com/acme/tool".to_owned())
        );
        assert_eq!(
            parse_package_clause("//go:build unix\n\n/* license */\npackage main\n"),
            Some("main".to_owned())
        );
        assert_eq!(
            parse_package_clause("package helper\n"),
            Some("helper".to_owned())
        );
    }
}
