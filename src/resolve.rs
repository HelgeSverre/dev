use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::candidate::{Candidate, SelectionPolicy};
use crate::query::rank::compare_hinted;
use crate::query::{match_candidate, normalize_query, MatchClass, MatchStrategy, QueryMatch};
use crate::score::{AUTO_FLOOR, CLEAR_WINNER_MARGIN};

const IDENTITY_QUALITY_MARGIN: u16 = 40;
const QUERY_POINTS_MARGIN: i32 = 8;
const AUTOMATIC_IDENTITY_QUALITY: u16 = 860;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    Resolved,
    Ambiguous,
    HintNoMatch,
    NoCandidates,
    Remembered,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionReason {
    UniqueAutomaticCandidate,
    ClearStructuralWinner,
    UniqueIdentityMatch,
    ScopedStructuralWinner,
    ForcedPicker,
    LowConfidence,
    CloseCandidates,
    NoAutomaticCandidates,
    HintNoMatch,
    NoCandidates,
}

#[derive(Clone, Debug)]
pub struct RankedCandidate {
    pub candidate: Candidate,
    pub query: QueryMatch,
    pub finalist: bool,
}

#[derive(Clone, Debug)]
pub struct Resolution {
    pub status: ResolutionStatus,
    pub reason: ResolutionReason,
    pub selected: Option<usize>,
    pub candidates: Vec<RankedCandidate>,
}

impl Resolution {
    #[must_use]
    pub fn selected_candidate(&self) -> Option<&Candidate> {
        self.selected.map(|index| &self.candidates[index].candidate)
    }
}

#[must_use]
pub fn resolve(
    candidates: Vec<Candidate>,
    hints: &[String],
    chaos: u8,
    force_pick: bool,
) -> Resolution {
    if candidates.is_empty() {
        return Resolution {
            status: ResolutionStatus::NoCandidates,
            reason: ResolutionReason::NoCandidates,
            selected: None,
            candidates: Vec::new(),
        };
    }
    if hints.is_empty() {
        resolve_unhinted(candidates, force_pick)
    } else {
        resolve_hinted(candidates, hints, chaos, force_pick)
    }
}

fn resolve_unhinted(candidates: Vec<Candidate>, force_pick: bool) -> Resolution {
    let mut candidates = candidates
        .into_iter()
        .map(|candidate| RankedCandidate {
            candidate,
            query: QueryMatch::default(),
            finalist: false,
        })
        .collect::<Vec<_>>();
    candidates.sort_by(compare_unhinted);
    if force_pick {
        return ambiguous(candidates, ResolutionReason::ForcedPicker);
    }

    let automatic = candidates
        .iter()
        .enumerate()
        .filter(|(_, ranked)| {
            ranked.candidate.availability.is_available()
                && ranked.candidate.selection == SelectionPolicy::Automatic
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let Some(&top) = automatic.first() else {
        return ambiguous(candidates, ResolutionReason::NoAutomaticCandidates);
    };
    if candidates[top].candidate.structural_points < AUTO_FLOOR {
        return ambiguous(candidates, ResolutionReason::LowConfidence);
    }
    if automatic.len() == 1 {
        return resolved(candidates, top, ResolutionReason::UniqueAutomaticCandidate);
    }
    let second = automatic[1];
    if candidates[top].candidate.structural_points - candidates[second].candidate.structural_points
        > CLEAR_WINNER_MARGIN
    {
        resolved(candidates, top, ResolutionReason::ClearStructuralWinner)
    } else {
        ambiguous(candidates, ResolutionReason::CloseCandidates)
    }
}

fn resolve_hinted(
    candidates: Vec<Candidate>,
    hints: &[String],
    chaos: u8,
    force_pick: bool,
) -> Resolution {
    let query = normalize_query(hints);
    let mut candidates = candidates
        .into_iter()
        .map(|candidate| {
            let query_match = match_candidate(&candidate, &query, chaos);
            RankedCandidate {
                candidate,
                query: query_match,
                finalist: false,
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        compare_hinted(
            (&left.candidate, &left.query),
            (&right.candidate, &right.query),
        )
    });
    if force_pick {
        return ambiguous(candidates, ResolutionReason::ForcedPicker);
    }

    let matched = candidates
        .iter()
        .enumerate()
        .filter(|(_, ranked)| ranked.query.matched_meaningful_terms > 0)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let Some(&top) = matched.first() else {
        return Resolution {
            status: ResolutionStatus::HintNoMatch,
            reason: ResolutionReason::HintNoMatch,
            selected: None,
            candidates,
        };
    };

    let top_class = candidates[top].query.highest_class;
    let top_quality = candidates[top].query.best_identity_quality;
    let top_points = candidates[top].query.total_points;
    let finalists = matched
        .into_iter()
        .filter(|&index| {
            let matched = &candidates[index].query;
            matched.highest_class == top_class
                && (top_class != Some(MatchClass::Identity)
                    || top_quality.saturating_sub(matched.best_identity_quality)
                        <= IDENTITY_QUALITY_MARGIN)
                && top_points - matched.total_points <= QUERY_POINTS_MARGIN
        })
        .collect::<Vec<_>>();
    for &index in &finalists {
        candidates[index].finalist = true;
    }

    let top_candidate = &candidates[top].candidate;
    let top_match = &candidates[top].query;
    let identity_gate = top_match.best_identity_quality >= AUTOMATIC_IDENTITY_QUALITY;
    let exact_scope_gate = top_candidate.selection == SelectionPolicy::Automatic
        && has_exact_scope_match(top_match)
        && structurally_clear_within_scope(&candidates, top);
    let policy_gate = match top_candidate.selection {
        SelectionPolicy::Automatic => true,
        SelectionPolicy::ExplicitHint => identity_gate,
        SelectionPolicy::Confirm => false,
    };
    if top_candidate.availability.is_available()
        && finalists.len() == 1
        && top_match.matched_meaningful_terms > 0
        && (identity_gate || exact_scope_gate)
        && policy_gate
    {
        let reason = if identity_gate {
            ResolutionReason::UniqueIdentityMatch
        } else {
            ResolutionReason::ScopedStructuralWinner
        };
        resolved(candidates, top, reason)
    } else {
        ambiguous(candidates, ResolutionReason::CloseCandidates)
    }
}

fn has_exact_scope_match(query: &QueryMatch) -> bool {
    query.terms.iter().any(|term| {
        term.class == MatchClass::Scope
            && matches!(
                term.strategy,
                MatchStrategy::ExactSegment | MatchStrategy::ExactCompact
            )
    })
}

fn structurally_clear_within_scope(candidates: &[RankedCandidate], top: usize) -> bool {
    let second = candidates.iter().enumerate().find(|(index, candidate)| {
        *index != top
            && candidate.query.highest_class == candidates[top].query.highest_class
            && candidate.candidate.selection == SelectionPolicy::Automatic
    });
    second.is_none_or(|(_, second)| {
        candidates[top].candidate.structural_points - second.candidate.structural_points
            > CLEAR_WINNER_MARGIN
    })
}

fn compare_unhinted(left: &RankedCandidate, right: &RankedCandidate) -> Ordering {
    right
        .candidate
        .availability
        .is_available()
        .cmp(&left.candidate.availability.is_available())
        .then_with(|| {
            (right.candidate.selection == SelectionPolicy::Automatic)
                .cmp(&(left.candidate.selection == SelectionPolicy::Automatic))
        })
        .then_with(|| {
            right
                .candidate
                .structural_points
                .cmp(&left.candidate.structural_points)
        })
        .then_with(|| {
            left.candidate
                .anchor_distance
                .cmp(&right.candidate.anchor_distance)
        })
        .then_with(|| left.candidate.action_key.cmp(&right.candidate.action_key))
        .then_with(|| left.candidate.id.cmp(&right.candidate.id))
}

fn resolved(
    candidates: Vec<RankedCandidate>,
    selected: usize,
    reason: ResolutionReason,
) -> Resolution {
    Resolution {
        status: ResolutionStatus::Resolved,
        reason,
        selected: Some(selected),
        candidates,
    }
}

fn ambiguous(candidates: Vec<RankedCandidate>, reason: ResolutionReason) -> Resolution {
    Resolution {
        status: ResolutionStatus::Ambiguous,
        reason,
        selected: None,
        candidates,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::candidate::{Availability, SearchDocument};
    use crate::intent::Intent;

    use super::*;

    fn available(name: &str, points: i32, policy: SelectionPolicy) -> Candidate {
        let mut candidate = Candidate::new(
            format!("test:{name}"),
            "node",
            Intent::Run,
            name,
            "true",
            Vec::new(),
            PathBuf::from("/tmp"),
            points,
            policy,
        );
        candidate.label = name.to_owned();
        candidate.search = SearchDocument {
            identities: vec![name.to_owned()],
            ..SearchDocument::default()
        };
        candidate.availability = Availability::Available {
            resolved_program: PathBuf::from("/usr/bin/true"),
        };
        candidate.structural_points = points;
        candidate
    }

    #[test]
    fn unmatched_hint_never_falls_back_to_default() {
        let result = resolve(
            vec![available("dev", 95, SelectionPolicy::Automatic)],
            &["purple".to_owned(), "lasagna".to_owned()],
            1,
            false,
        );
        assert_eq!(result.status, ResolutionStatus::HintNoMatch);
    }

    #[test]
    fn unrelated_low_tier_candidate_does_not_change_clear_winner() {
        let baseline = resolve(
            vec![available("dev", 95, SelectionPolicy::Automatic)],
            &[],
            0,
            false,
        );
        let widened = resolve(
            vec![
                available("dev", 95, SelectionPolicy::Automatic),
                available("refresh", 15, SelectionPolicy::ExplicitHint),
            ],
            &[],
            0,
            false,
        );
        assert_eq!(
            baseline
                .selected_candidate()
                .map(|candidate| &candidate.action_name),
            widened
                .selected_candidate()
                .map(|candidate| &candidate.action_name)
        );
    }

    #[test]
    fn confirm_candidate_never_auto_runs() {
        let result = resolve(
            vec![available("worker", 95, SelectionPolicy::Confirm)],
            &["worker".to_owned()],
            1,
            false,
        );
        assert_eq!(result.status, ResolutionStatus::Ambiguous);
    }
}
