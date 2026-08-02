use std::fmt::Write;

use crate::diagnostic::Diagnostic;
use crate::resolve::{Resolution, ResolutionStatus};
use crate::scan::{FileIndex, RootInfo};

use super::command_display;

#[must_use]
pub fn render(
    resolution: &Resolution,
    roots: &RootInfo,
    index: &FileIndex,
    diagnostics: &[Diagnostic],
) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "Resolution: {:?} ({:?})",
        resolution.status, resolution.reason
    );
    let _ = writeln!(
        output,
        "Package root: {}",
        display_optional(roots.package_root.as_deref())
    );
    let _ = writeln!(
        output,
        "Workspace root: {}",
        display_optional(roots.workspace_root.as_deref())
    );
    let _ = writeln!(output, "Scan root: {}", roots.scan_root.display());
    let _ = writeln!(
        output,
        "Scanned: {} structural, {} target entries{}",
        index.structural.len(),
        index.targets.len(),
        if index.truncated.is_empty() {
            ""
        } else {
            " (truncated)"
        }
    );

    for ranked in &resolution.candidates {
        let selected = resolution
            .selected_candidate()
            .is_some_and(|candidate| candidate.id == ranked.candidate.id);
        let marker = if selected {
            "●"
        } else if ranked.finalist {
            "◆"
        } else {
            "○"
        };
        let _ = writeln!(
            output,
            "\n{marker} {}  [{} structural, {} query]",
            ranked.candidate.label, ranked.candidate.structural_points, ranked.query.total_points
        );
        let _ = writeln!(
            output,
            "  command: {}",
            command_display::diagnostic(&ranked.candidate, &[])
        );
        let _ = writeln!(output, "  cwd: {}", ranked.candidate.cwd.display());
        let _ = writeln!(
            output,
            "  source: {}; layer: {:?}; policy: {:?}; availability: {:?}",
            ranked.candidate.source,
            ranked.candidate.layer,
            ranked.candidate.selection,
            ranked.candidate.availability
        );
        if !ranked.query.terms.is_empty() {
            let _ = writeln!(
                output,
                "  query coverage: {}/{}",
                ranked.query.matched_meaningful_terms, ranked.query.meaningful_terms
            );
            for matched in &ranked.query.terms {
                let _ = writeln!(
                    output,
                    "    {:+4} {:?}/{:?}: {:?} -> {:?}",
                    matched.points,
                    matched.class,
                    matched.strategy,
                    matched.hint,
                    matched.candidate_value
                );
            }
        }
        for evidence in &ranked.candidate.evidence {
            let _ = writeln!(
                output,
                "    {:+4} {:?}: {}{}",
                evidence.points,
                evidence.kind,
                evidence.reason,
                evidence
                    .source
                    .as_ref()
                    .map_or_else(String::new, |path| format!(" ({})", path.display()))
            );
        }
    }
    if !diagnostics.is_empty() {
        let _ = writeln!(output, "\nDiagnostics:");
        for diagnostic in diagnostics {
            let _ = writeln!(
                output,
                "  {:?} {}: {}{}",
                diagnostic.severity,
                diagnostic.detector,
                diagnostic.message,
                diagnostic
                    .source
                    .as_ref()
                    .map_or_else(String::new, |path| format!(" ({})", path.display()))
            );
        }
    }
    if resolution.status == ResolutionStatus::HintNoMatch {
        let _ = writeln!(
            output,
            "\nNo meaningful hint matched; the unhinted default was not selected."
        );
    }
    output
}

#[must_use]
pub fn list(resolution: &Resolution) -> String {
    resolution
        .candidates
        .iter()
        .map(|ranked| {
            format!(
                "{:>4}  {:<14}  {}",
                ranked.candidate.structural_points,
                format!("{:?}", ranked.candidate.selection),
                command_display::diagnostic(&ranked.candidate, &[])
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn display_optional(path: Option<&std::path::Path>) -> String {
    path.map_or_else(|| "—".to_owned(), |path| path.display().to_string())
}
