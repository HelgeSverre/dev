use std::fmt::Write;

use crate::diagnostic::Diagnostic;
use crate::intent::{Intent, Invocation};
use crate::resolve::{Resolution, ResolutionStatus};
use crate::scan::{FileIndex, RootInfo};

use super::command_display;

#[must_use]
pub fn candidate_table(
    resolution: &Resolution,
    invocation: &Invocation,
    roots: &RootInfo,
    index: &FileIndex,
    diagnostics: &[Diagnostic],
) -> String {
    if resolution.status == ResolutionStatus::NoCandidates {
        return no_candidates(invocation, roots, index, diagnostics);
    }
    let mut output = String::new();
    let _ = writeln!(
        output,
        "dev: resolution requires an interactive choice ({:?})",
        resolution.reason
    );
    for ranked in &resolution.candidates {
        let _ = writeln!(
            output,
            "  {:>4}  {}",
            ranked.candidate.structural_points + ranked.query.total_points,
            command_display::diagnostic(&ranked.candidate, &[])
        );
    }
    if resolution.status == ResolutionStatus::HintNoMatch {
        let _ = writeln!(
            output,
            "  no meaningful hint matched; the unhinted default was not selected"
        );
    }
    output
}

fn no_candidates(
    invocation: &Invocation,
    roots: &RootInfo,
    index: &FileIndex,
    diagnostics: &[Diagnostic],
) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "dev: nothing runnable found for {:?} in {}\n",
        invocation.intent,
        invocation.target.path().display()
    );
    let _ = writeln!(
        output,
        "  package root:   {}",
        roots
            .package_root
            .as_deref()
            .map_or_else(|| "—".to_owned(), |path| path.display().to_string())
    );
    let _ = writeln!(
        output,
        "  workspace root: {}",
        roots
            .workspace_root
            .as_deref()
            .map_or_else(|| "—".to_owned(), |path| path.display().to_string())
    );
    let _ = writeln!(output, "  scan root:      {}", roots.scan_root.display());
    let _ = writeln!(
        output,
        "  scanned:        {} entries ({})",
        index.structural.len() + index.targets.len(),
        if index.truncated.is_empty() {
            "complete"
        } else {
            "truncated"
        }
    );
    let manifests = manifest_names(index);
    let _ = writeln!(
        output,
        "  found:          {}",
        if manifests.is_empty() {
            "—".to_owned()
        } else {
            manifests.join(", ")
        }
    );
    for diagnostic in diagnostics {
        let _ = writeln!(
            output,
            "  {}: {}{}",
            diagnostic.detector,
            diagnostic.message,
            diagnostic
                .source
                .as_ref()
                .map_or_else(String::new, |path| format!(" ({})", path.display()))
        );
    }
    let alternatives = match invocation.intent {
        Intent::Run => ["test", "build"],
        Intent::Build => ["run", "test"],
        Intent::Test => ["run", "build"],
    };
    let _ = writeln!(output, "\n  Try:");
    for alternative in alternatives {
        let _ = writeln!(
            output,
            "    dev {alternative} --at {}",
            invocation.target.path().display()
        );
    }
    let _ = writeln!(
        output,
        "    dev {} --pick --at {}",
        invocation.intent,
        invocation.target.path().display()
    );
    output
}

fn manifest_names(index: &FileIndex) -> Vec<String> {
    let mut names = index
        .all_entries()
        .filter(|entry| {
            crate::registry::markers()
                .iter()
                .any(|marker| marker.pattern.matches(&entry.relative_path))
        })
        .filter_map(|entry| entry.relative_path.file_name()?.to_str().map(str::to_owned))
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}
