use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{Availability, Evidence, EvidenceKind, SearchDocument, SelectionPolicy};
use crate::intent::Intent;
use crate::registry::{CMAKE_SOURCE, CMAKE_TOOL, CTEST_TOOL};
use crate::scan::IndexedFileType;

use super::{CandidateBuilder, Detection, Detector, ScanCtx};

pub struct CmakeDetector;

impl Detector for CmakeDetector {
    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let has_cmake = context.index.all_entries().any(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "CMakeLists.txt")
        });
        if !has_cmake {
            return Detection::default();
        }
        let build_dir = find_build_directory(context);
        let mut candidates = Vec::new();
        if let Some(ref dir) = build_dir {
            match context.invocation.intent {
                Intent::Run => {}
                Intent::Build => {
                    candidates.push(cmake_build_candidate(
                        context,
                        dir,
                        SelectionPolicy::Automatic,
                        95,
                    ));
                }
                Intent::Test => {
                    candidates.push(ctest_candidate(
                        context,
                        dir,
                        SelectionPolicy::Automatic,
                        95,
                    ));
                }
            }
        } else {
            let scope = context.roots.scan_root.file_name().map_or_else(
                || "cmake-project".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            let description =
                "CMakeLists.txt found but no build directory; run `cmake -S . -B build` first";
            let candidate = CandidateBuilder::tool_default(
                CMAKE_SOURCE,
                Intent::Build,
                context.roots.scan_root.to_path_buf(),
                "build",
            )
            .action_key(format!("cmake:{}:unconfigured", scope))
            .tool(CMAKE_TOOL)
            .args([OsString::from("--build"), OsString::from("build")])
            .cwd(context.roots.scan_root.to_path_buf())
            .selection(SelectionPolicy::ExplicitHint)
            .base_points(20)
            .availability(Availability::UnsupportedHost {
                reason: format!(
                    "no CMake build directory found in {}",
                    context.roots.scan_root.display()
                ),
            })
            .label("CMake build")
            .description(description)
            .evidence(Evidence {
                kind: EvidenceKind::Manifest,
                reason: "project contains CMakeLists.txt".to_owned(),
                points: 0,
                source: Some(PathBuf::from("CMakeLists.txt")),
            })
            .search(SearchDocument {
                identities: vec!["build".to_owned(), "cmake".to_owned()],
                target_paths: vec![PathBuf::from("CMakeLists.txt")],
                scopes: vec![scope],
                tags: vec!["cmake".to_owned(), "c++".to_owned(), "cpp".to_owned()],
                text: vec![description.to_owned()],
            })
            .build()
            .expect("CMake unconfigured candidate registration is valid");
            candidates.push(candidate);
        }
        Detection {
            candidates,
            diagnostics: Vec::new(),
        }
    }
}

fn find_build_directory(context: &ScanCtx<'_>) -> Option<PathBuf> {
    let mut entries: Vec<_> = context
        .index
        .all_entries()
        .filter(|entry| {
            entry.file_type == IndexedFileType::File
                && entry
                    .relative_path
                    .file_name()
                    .is_some_and(|name| name == "CMakeCache.txt")
        })
        .map(|entry| entry.relative_path.clone())
        .collect();
    entries.sort();
    entries.first().map(|entry| {
        context
            .roots
            .scan_root
            .join(entry.parent().unwrap_or(Path::new(".")))
    })
}

fn cmake_build_candidate(
    context: &ScanCtx<'_>,
    build_dir: &Path,
    selection: SelectionPolicy,
    base_points: i32,
) -> crate::candidate::Candidate {
    let scope = context.roots.scan_root.file_name().map_or_else(
        || "cmake-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let build_relative = build_dir
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(build_dir)
        .as_os_str()
        .to_owned();
    let description = "CMake build from configured build directory";
    CandidateBuilder::tool_default(
        CMAKE_SOURCE,
        Intent::Build,
        context.roots.scan_root.to_path_buf(),
        "build",
    )
    .action_key(format!("cmake:{}:build", scope))
    .tool(CMAKE_TOOL)
    .args([OsString::from("--build"), build_relative])
    .cwd(context.roots.scan_root.to_path_buf())
    .selection(selection)
    .base_points(base_points)
    .label("CMake build")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: "project contains CMakeLists.txt with a configured build directory".to_owned(),
        points: 0,
        source: Some(PathBuf::from("CMakeLists.txt")),
    })
    .search(SearchDocument {
        identities: vec!["build".to_owned(), "cmake".to_owned()],
        target_paths: vec![PathBuf::from("CMakeLists.txt")],
        scopes: vec![scope],
        tags: vec!["cmake".to_owned(), "c++".to_owned(), "cpp".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("CMake build candidate registration is valid")
}

fn ctest_candidate(
    context: &ScanCtx<'_>,
    build_dir: &Path,
    selection: SelectionPolicy,
    base_points: i32,
) -> crate::candidate::Candidate {
    let scope = context.roots.scan_root.file_name().map_or_else(
        || "cmake-project".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let build_relative = build_dir
        .strip_prefix(&context.roots.scan_root)
        .unwrap_or(build_dir)
        .as_os_str()
        .to_owned();
    let description = "Run project tests via CTest";
    CandidateBuilder::tool_default(
        CMAKE_SOURCE,
        Intent::Test,
        context.roots.scan_root.to_path_buf(),
        "test",
    )
    .action_key(format!("cmake:{}:test", scope))
    .tool(CTEST_TOOL)
    .args([OsString::from("--test-dir"), build_relative])
    .cwd(context.roots.scan_root.to_path_buf())
    .selection(selection)
    .base_points(base_points)
    .label("CMake test")
    .description(description)
    .evidence(Evidence {
        kind: EvidenceKind::Manifest,
        reason: "project contains CMakeLists.txt with a configured build directory".to_owned(),
        points: 0,
        source: Some(PathBuf::from("CMakeLists.txt")),
    })
    .search(SearchDocument {
        identities: vec!["test".to_owned(), "ctest".to_owned(), "cmake".to_owned()],
        target_paths: vec![PathBuf::from("CMakeLists.txt")],
        scopes: vec![scope],
        tags: vec!["cmake".to_owned(), "c++".to_owned(), "cpp".to_owned()],
        text: vec![description.to_owned()],
    })
    .build()
    .expect("CTest candidate registration is valid")
}
