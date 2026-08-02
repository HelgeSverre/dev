use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::candidate::{
    Candidate, CandidateOrigin, Evidence, EvidenceKind, Lifecycle, SearchDocument, SelectionPolicy,
};
use crate::intent::Intent;
use crate::scan::{IndexEntry, IndexedFileType};

use super::script::{read_shebang, Shebang};
use super::target::explicitly_anchored;
use super::{Detection, Detector, ScanCtx, TargetRunner};

pub struct ShellDetector;

impl Detector for ShellDetector {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn synonyms(&self) -> &'static [&'static str] {
        &["shell", "script", "sh"]
    }

    fn detect(&self, context: &ScanCtx<'_>) -> Detection {
        let mut candidates = context
            .index
            .all_entries()
            .filter_map(|entry| discovery_candidate(context, entry))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.action_key.cmp(&right.action_key));
        candidates.dedup_by(|left, right| left.action_key == right.action_key);
        Detection {
            candidates,
            diagnostics: Vec::new(),
        }
    }
}

impl TargetRunner for ShellDetector {
    fn supports(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> bool {
        if context.invocation.intent != Intent::Run
            || target.file_type == IndexedFileType::Directory
            || claimed_by_language_detector(&target.relative_path)
        {
            return false;
        }
        let absolute = context.roots.scan_root.join(&target.relative_path);
        target.executable
            || read_shebang(&absolute).is_some()
            || target
                .relative_path
                .extension()
                .is_some_and(|value| value == "sh")
    }

    fn candidate(&self, target: &IndexEntry, context: &ScanCtx<'_>) -> Option<Candidate> {
        let absolute = context.roots.scan_root.join(&target.relative_path);
        let explicit = explicitly_anchored(target, context);
        script_candidate(
            &absolute,
            &target.relative_path,
            Intent::Run,
            target.executable,
            read_shebang(&absolute),
            if explicit { 90 } else { 20 },
            if explicit {
                SelectionPolicy::Automatic
            } else {
                SelectionPolicy::ExplicitHint
            },
            CandidateOrigin::Synthetic,
        )
    }
}

fn discovery_candidate(context: &ScanCtx<'_>, entry: &IndexEntry) -> Option<Candidate> {
    if entry.file_type != IndexedFileType::File {
        return None;
    }
    let filename = entry.relative_path.file_name()?.to_str()?;
    let conventional_intent = match filename {
        "run.sh" | "dev.sh" | "start.sh" => Some(Intent::Run),
        "build.sh" => Some(Intent::Build),
        "test.sh" => Some(Intent::Test),
        _ => None,
    };
    let in_discovery_directory = entry
        .relative_path
        .components()
        .next()
        .is_some_and(|component| {
            component.as_os_str() == "scripts" || component.as_os_str() == "bin"
        });
    let hinted = !context.invocation.hints.is_empty();
    if claimed_by_language_detector(&entry.relative_path)
        && (!in_discovery_directory || !entry.executable || hinted)
    {
        return None;
    }
    let eligible = conventional_intent == Some(context.invocation.intent)
        || (context.invocation.intent == Intent::Run && in_discovery_directory && entry.executable);
    if !eligible {
        return None;
    }
    let absolute = context.roots.scan_root.join(&entry.relative_path);
    let shebang = read_shebang(&absolute);
    if !entry.executable
        && shebang.is_none()
        && entry
            .relative_path
            .extension()
            .is_none_or(|value| value != "sh")
    {
        return None;
    }
    let conventional = conventional_intent == Some(context.invocation.intent);
    let discovered_executable =
        context.invocation.intent == Intent::Run && in_discovery_directory && entry.executable;
    script_candidate(
        &absolute,
        &entry.relative_path,
        context.invocation.intent,
        entry.executable,
        shebang,
        if conventional { 75 } else { 20 },
        if conventional {
            SelectionPolicy::Automatic
        } else {
            SelectionPolicy::ExplicitHint
        },
        if conventional || discovered_executable {
            CandidateOrigin::Conventional
        } else {
            CandidateOrigin::Synthetic
        },
    )
}

fn claimed_by_language_detector(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| matches!(extension.to_str(), Some("dart" | "php" | "py" | "zig")))
}

#[allow(clippy::too_many_arguments)]
fn script_candidate(
    absolute: &Path,
    relative_to_scan: &Path,
    intent: Intent,
    executable: bool,
    shebang: Option<Shebang>,
    base_points: i32,
    selection: SelectionPolicy,
    origin: CandidateOrigin,
) -> Option<Candidate> {
    let directory = absolute.parent().unwrap_or(Path::new(".")).to_path_buf();
    let filename = absolute.file_name()?.to_os_string();
    let stem = absolute.file_stem().map_or_else(
        || filename.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let (program, mut args, runner_reason) = if executable {
        (
            PathBuf::from(".").join(&filename).into_os_string(),
            Vec::new(),
            shebang.as_ref().map_or_else(
                || "the executable bit".to_owned(),
                |value| format!("executable shebang `{}`", value.display),
            ),
        )
    } else if let Some(shebang) = shebang {
        (
            shebang.program,
            shebang.arguments,
            format!("declared shebang `{}`", shebang.display),
        )
    } else {
        (
            OsString::from("sh"),
            Vec::new(),
            "the .sh interpreter fallback".to_owned(),
        )
    };
    if !executable {
        args.push(filename.clone());
    }
    let mut candidate = Candidate::new(
        format!(
            "shell:{}:{}",
            intent,
            relative_to_scan.to_string_lossy().replace(['/', '\\'], ":")
        ),
        "shell",
        intent,
        &stem,
        program,
        args,
        directory,
        base_points,
        selection,
    );
    candidate.origin = origin;
    candidate.lifecycle = Lifecycle::Finite;
    candidate.label = format!("Script {}", relative_to_scan.display());
    candidate.description = format!("Script target using {runner_reason}");
    candidate.evidence.push(Evidence {
        kind: if origin == CandidateOrigin::Conventional {
            EvidenceKind::Convention
        } else {
            EvidenceKind::Rule
        },
        reason: format!("selected {runner_reason} for script target"),
        points: 0,
        source: Some(relative_to_scan.to_path_buf()),
    });
    candidate.search = SearchDocument {
        identities: vec![stem, filename.to_string_lossy().into_owned()],
        target_paths: vec![PathBuf::from(&filename), relative_to_scan.to_path_buf()],
        scopes: relative_to_scan
            .parent()
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        tags: vec!["shell".to_owned(), "script".to_owned()],
        text: vec![candidate.description.clone()],
    };
    Some(candidate)
}
