use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, CandidateOrigin, Evidence, EvidenceKind, SearchDocument, SelectionPolicy,
};
use crate::intent::Intent;
use crate::registry::PHP_FILE_SOURCE;
use crate::scan::{IndexEntry, IndexedFileType};

use super::target::{explicitly_anchored, target_scope};
use super::{CandidateBuilder, Detection, Detector, ScanCtx, TargetRunner};

pub struct PhpFileDetector;

impl Detector for PhpFileDetector {
    fn detect(&self, _context: &ScanCtx<'_>) -> Detection {
        Detection::default()
    }
}

impl TargetRunner for PhpFileDetector {
    fn supports(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
        context.invocation.intent == Intent::Run && is_php_file(target)
    }

    fn candidate(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        Some(file_candidate(
            &absolute,
            &target.relative_path,
            target.executable,
            explicitly_anchored(target, context),
        ))
    }
}

fn file_candidate(
    absolute: &Path,
    relative_to_scan: &Path,
    is_executable: bool,
    explicitly_anchored: bool,
) -> Candidate {
    let directory = absolute.parent().unwrap_or(Path::new(".")).to_path_buf();
    let filename = absolute
        .file_name()
        .map_or_else(|| OsString::from("script.php"), std::ffi::OsStr::to_owned);
    let stem = absolute.file_stem().map_or_else(
        || "php-script".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let direct = is_executable && has_php_shebang(absolute);
    let (program, args, runner_reason) = if direct {
        (
            PathBuf::from(".").join(&filename).into_os_string(),
            Vec::new(),
            "executable PHP shebang",
        )
    } else {
        (
            OsString::from("php"),
            vec![filename.clone()],
            "resolved PHP interpreter",
        )
    };
    let description = format!("Standalone PHP target using {runner_reason}");
    CandidateBuilder::direct_target(PHP_FILE_SOURCE, Intent::Run, directory.clone(), &stem)
        .action_key(format!(
            "php-file:{}",
            relative_to_scan.to_string_lossy().replace(['/', '\\'], ":")
        ))
        .program_path(program)
        .args(args)
        .cwd(directory)
        .selection(if explicitly_anchored {
            SelectionPolicy::Automatic
        } else {
            SelectionPolicy::ExplicitHint
        })
        .base_points(if explicitly_anchored { 90 } else { 25 })
        .origin(if explicitly_anchored {
            CandidateOrigin::Declared
        } else {
            CandidateOrigin::Synthetic
        })
        .label(format!("PHP file {}", relative_to_scan.display()))
        .description(&description)
        .evidence(Evidence {
            kind: EvidenceKind::Rule,
            reason: format!("selected {runner_reason} for PHP target"),
            points: 0,
            source: Some(relative_to_scan.to_path_buf()),
        })
        .search(SearchDocument {
            identities: vec![stem, filename.to_string_lossy().into_owned()],
            target_paths: vec![PathBuf::from(&filename), relative_to_scan.to_path_buf()],
            scopes: vec![target_scope(relative_to_scan)],
            tags: vec!["php".to_owned()],
            text: vec![description],
        })
        .build()
        .expect("PHP file candidate registration is valid")
}

fn is_php_file(entry: &IndexEntry) -> bool {
    entry.file_type != IndexedFileType::Directory
        && entry
            .relative_path
            .extension()
            .is_some_and(|extension| extension == "php")
}

fn has_php_shebang(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut bytes = [0_u8; 512];
    let Ok(read) = file.read(&mut bytes) else {
        return false;
    };
    let first_line = bytes[..read]
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    first_line.starts_with(b"#!")
        && String::from_utf8_lossy(first_line)
            .to_ascii_lowercase()
            .contains("php")
}
