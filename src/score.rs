use std::collections::HashSet;
use std::path::Path;

use crate::candidate::{Availability, Candidate, Evidence, EvidenceKind};
use crate::intent::Target;
use crate::path::resolve_program;

pub const AUTO_FLOOR: i32 = 30;
pub const CLEAR_WINNER_MARGIN: i32 = 15;
pub const SAME_DIR_POINTS: i32 = 30;
pub const EXACT_FILE_POINTS: i32 = 40;
pub const MISSING_PROGRAM_POINTS: i32 = -50;

/// Finalize availability and structural score after candidate deduplication.
pub fn finalize(candidate: &mut Candidate, target: &Target) {
    candidate.anchor_distance = directory_distance(target.anchor_directory(), &candidate.cwd);
    if !matches!(candidate.availability, Availability::UnsupportedHost { .. }) {
        candidate.availability =
            resolve_program(&candidate.program, &candidate.cwd, &candidate.env);
    }
    candidate.evidence.retain(|evidence| {
        !matches!(
            evidence.kind,
            EvidenceKind::Proximity | EvidenceKind::Availability
        )
    });

    if let Some(points) = proximity_points(candidate.anchor_distance) {
        candidate.evidence.push(Evidence {
            kind: EvidenceKind::Proximity,
            reason: if candidate.anchor_distance == 0 {
                "same directory as target".to_owned()
            } else {
                format!("{} directory edges from target", candidate.anchor_distance)
            },
            points,
            source: Some(candidate.cwd.clone()),
        });
    }

    if let Target::File(path) = target {
        let canonical_target = path.canonicalize().ok();
        let directly_targets_file = candidate.search.target_paths.iter().any(|target_path| {
            let logical = candidate.cwd.join(target_path);
            target_path == path
                || logical == *path
                || canonical_target.as_ref().is_some_and(|canonical_target| {
                    logical.canonicalize().ok().as_ref() == Some(canonical_target)
                })
        });
        if directly_targets_file {
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Proximity,
                reason: "directly targets the anchored file".to_owned(),
                points: EXACT_FILE_POINTS,
                source: Some(path.clone()),
            });
        }
    }

    if !candidate.availability.is_available() {
        candidate.evidence.push(Evidence {
            kind: EvidenceKind::Availability,
            reason: availability_reason(&candidate.availability),
            points: MISSING_PROGRAM_POINTS,
            source: None,
        });
    }
    recompute(candidate);
    candidate.refresh_id();
}

/// Recompute structural points from unique evidence.
pub fn recompute(candidate: &mut Candidate) {
    let mut seen = HashSet::new();
    candidate.evidence.retain(|evidence| {
        seen.insert((
            evidence.kind,
            evidence.reason.clone(),
            evidence.source.clone(),
        ))
    });
    candidate.structural_points = candidate.base_points
        + candidate
            .evidence
            .iter()
            .map(|evidence| evidence.points)
            .sum::<i32>();
}

fn availability_reason(availability: &Availability) -> String {
    match availability {
        Availability::Available { .. } => "program is available".to_owned(),
        Availability::MissingProgram { program } => {
            format!("program `{}` is not available", program.to_string_lossy())
        }
        Availability::UnsupportedHost { reason } => reason.clone(),
    }
}

#[must_use]
pub fn directory_distance(left: &Path, right: &Path) -> usize {
    let left_components = left.components().collect::<Vec<_>>();
    let right_components = right.components().collect::<Vec<_>>();
    let common = left_components
        .iter()
        .zip(&right_components)
        .take_while(|(left, right)| left == right)
        .count();
    left_components.len() + right_components.len() - 2 * common
}

fn proximity_points(distance: usize) -> Option<i32> {
    match distance {
        0 => Some(SAME_DIR_POINTS),
        1 => Some(15),
        2 => Some(8),
        3 => Some(4),
        _ => None,
    }
}
