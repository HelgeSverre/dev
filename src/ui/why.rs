use std::fmt::Write;

use crate::diagnostic::Diagnostic;
use crate::resolve::{Resolution, ResolutionStatus};
use crate::scan::{FileIndex, RootInfo};

use super::command_display;
use super::terminal_text;

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
    let _ = writeln!(
        output,
        "Scan root: {}",
        terminal_text(&roots.scan_root.to_string_lossy())
    );
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
            terminal_text(&ranked.candidate.label),
            ranked.candidate.structural_points,
            ranked.query.total_points
        );
        let _ = writeln!(
            output,
            "  command: {}",
            terminal_text(&command_display::diagnostic(&ranked.candidate, &[]))
        );
        let _ = writeln!(
            output,
            "  cwd: {}",
            terminal_text(&ranked.candidate.cwd.to_string_lossy())
        );
        let metadata = format!(
            "source: {}; layer: {:?}; policy: {:?}; availability: {:?}",
            ranked.candidate.source,
            ranked.candidate.layer,
            ranked.candidate.selection,
            ranked.candidate.availability
        );
        let _ = writeln!(output, "  {}", terminal_text(&metadata));
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
                    terminal_text(&matched.hint),
                    terminal_text(&matched.candidate_value)
                );
            }
        }
        for evidence in &ranked.candidate.evidence {
            let _ = writeln!(
                output,
                "    {:+4} {:?}: {}{}",
                evidence.points,
                evidence.kind,
                terminal_text(&evidence.reason),
                evidence.source.as_ref().map_or_else(String::new, |path| {
                    format!(" ({})", terminal_text(&path.to_string_lossy()))
                })
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
                terminal_text(&diagnostic.message),
                diagnostic.source.as_ref().map_or_else(String::new, |path| {
                    format!(" ({})", terminal_text(&path.to_string_lossy()))
                })
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
                terminal_text(&command_display::diagnostic(&ranked.candidate, &[]))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn display_optional(path: Option<&std::path::Path>) -> String {
    path.map_or_else(
        || "—".to_owned(),
        |path| terminal_text(&path.to_string_lossy()),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::candidate::{Availability, Candidate, Evidence, EvidenceKind, SelectionPolicy};
    use crate::diagnostic::Diagnostic;
    use crate::intent::{Intent, Target};
    use crate::query::QueryMatch;
    use crate::registry::{NODE, NODE_SOURCE};
    use crate::resolve::{RankedCandidate, Resolution, ResolutionReason, ResolutionStatus};
    use crate::scan::{resolve_roots, FileIndex, ScanOptions};

    use super::*;

    #[test]
    fn human_readable_diagnostics_replace_project_control_characters() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let target = Target::Directory(temp.path().to_path_buf());
        let roots = resolve_roots(&target);
        let index = FileIndex::build(&roots, ScanOptions::default());
        let mut candidate = Candidate::new(
            "run:unsafe",
            NODE,
            NODE_SOURCE,
            Intent::Run,
            "unsafe",
            "node",
            Vec::new(),
            temp.path().to_path_buf(),
            50,
            SelectionPolicy::Automatic,
        );
        candidate.label = "deploy\u{1b}]52;clipboard\u{7}".to_owned();
        candidate.availability = Availability::Available {
            resolved_program: PathBuf::from("/tmp/tool\u{1b}[2J"),
        };
        candidate.evidence.push(Evidence {
            kind: EvidenceKind::Manifest,
            reason: "recipe\nspoof".to_owned(),
            points: 10,
            source: None,
        });
        let resolution = Resolution {
            status: ResolutionStatus::Resolved,
            reason: ResolutionReason::UniqueAutomaticCandidate,
            selected: Some(0),
            candidates: vec![RankedCandidate {
                candidate,
                query: QueryMatch::default(),
                finalist: true,
            }],
        };
        let diagnostics = [Diagnostic::warning(NODE, "warning\u{1b}[H", None)];

        let output = render(&resolution, &roots, &index, &diagnostics);
        assert!(!output.contains('\u{1b}'));
        assert!(!output.contains("recipe\nspoof"));
        assert!(output.contains("deploy�]52;clipboard�"));
        assert!(output.contains("recipe�spoof"));
        Ok(())
    }
}
