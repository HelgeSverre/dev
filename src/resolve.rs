use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::candidate::{
    Candidate, CandidateOrigin, CommandLayer, Evidence, EvidenceKind, SelectionPolicy,
};
use crate::query::rank::compare_hinted;
use crate::query::{
    match_candidate, normalize_query, MatchClass, MatchStrategy, QueryMatch, SearchField,
};
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
    RememberedChoice,
    RememberedCommandChanged,
    RememberedActionMissing,
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
        resolve_unhinted(apply_facade_dominance(candidates), force_pick)
    } else {
        resolve_hinted(candidates, hints, chaos, force_pick)
    }
}

fn apply_facade_dominance(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    let dominant = candidates
        .iter()
        .filter(|candidate| {
            candidate.availability.is_available()
                && candidate.selection == SelectionPolicy::Automatic
                && candidate.origin == CandidateOrigin::Declared
                && candidate.layer != CommandLayer::DirectTarget
                && canonical_identity(candidate.intent, &candidate.action_name)
        })
        .map(|candidate| {
            (
                candidate.scope_root.clone(),
                candidate.intent,
                layer_precedence(candidate.layer),
                candidate.layer,
            )
        })
        .fold(
            std::collections::BTreeMap::new(),
            |mut dominant, (scope, intent, precedence, layer)| {
                dominant
                    .entry((scope, intent))
                    .and_modify(|current: &mut (u8, CommandLayer)| {
                        if precedence > current.0 {
                            *current = (precedence, layer);
                        }
                    })
                    .or_insert((precedence, layer));
                dominant
            },
        );

    for candidate in &mut candidates {
        let Some((precedence, layer)) =
            dominant.get(&(candidate.scope_root.clone(), candidate.intent))
        else {
            continue;
        };
        if candidate.selection == SelectionPolicy::Automatic
            && candidate.layer != CommandLayer::DirectTarget
            && layer_precedence(candidate.layer) < *precedence
        {
            candidate.selection = SelectionPolicy::ExplicitHint;
            candidate.evidence.push(Evidence {
                kind: EvidenceKind::Rule,
                reason: format!("demoted by canonical same-scope {layer:?} project interface"),
                points: 0,
                source: None,
            });
        }
    }
    candidates
}

const fn layer_precedence(layer: CommandLayer) -> u8 {
    match layer {
        CommandLayer::ProjectFacade => 3,
        CommandLayer::EcosystemTask => 2,
        CommandLayer::ToolDefault => 1,
        CommandLayer::DirectTarget => 0,
    }
}

fn canonical_identity(intent: crate::intent::Intent, action: &str) -> bool {
    match intent {
        crate::intent::Intent::Run => {
            matches!(action, "run" | "dev" | "start" | "serve" | "watch")
        }
        crate::intent::Intent::Build => {
            matches!(
                action,
                "build" | "all" | "compile" | "bundle" | "assemble" | "package"
            )
        }
        crate::intent::Intent::Test => matches!(action, "test" | "check" | "verify"),
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
            && term.field == SearchField::Scope
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
    use crate::registry::{NODE, NODE_SOURCE};

    use super::*;

    fn available(name: &str, points: i32, policy: SelectionPolicy) -> Candidate {
        let mut candidate = Candidate::new(
            format!("test:{name}"),
            NODE,
            NODE_SOURCE,
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
    fn canonical_project_facade_demotes_lower_same_scope_layers() {
        let mut facade = available("test", 80, SelectionPolicy::Automatic);
        facade.intent = Intent::Test;
        facade.layer = CommandLayer::ProjectFacade;
        let mut ecosystem = available("test:unit", 95, SelectionPolicy::Automatic);
        ecosystem.intent = Intent::Test;
        ecosystem.action_name = "test".to_owned();
        ecosystem.layer = CommandLayer::EcosystemTask;

        let resolution = resolve(vec![ecosystem, facade], &[], 0, false);

        assert_eq!(resolution.status, ResolutionStatus::Resolved);
        assert_eq!(
            resolution
                .selected_candidate()
                .map(|candidate| candidate.layer),
            Some(CommandLayer::ProjectFacade)
        );
        assert!(resolution.candidates.iter().any(|ranked| {
            ranked.candidate.layer == CommandLayer::EcosystemTask
                && ranked.candidate.selection == SelectionPolicy::ExplicitHint
        }));
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

    #[test]
    fn explicit_hint_candidate_does_not_auto_run_from_scope_only() {
        let mut candidate = available("participant-sync", 80, SelectionPolicy::ExplicitHint);
        candidate.search.scopes.push("laravel".to_owned());
        let result = resolve(vec![candidate], &["laravel".to_owned()], 2, false);
        assert_eq!(result.status, ResolutionStatus::Ambiguous);
        assert_eq!(
            result.candidates[0].query.highest_class,
            Some(MatchClass::Scope)
        );
    }

    #[test]
    fn automatic_candidate_does_not_run_from_working_directory_only() {
        let candidate = available("dev", 95, SelectionPolicy::Automatic);
        let result = resolve(vec![candidate], &["tmp".to_owned()], 1, false);
        assert_eq!(result.status, ResolutionStatus::Ambiguous);
        assert_eq!(
            result.candidates[0].query.terms[0].field,
            SearchField::WorkingDirectory
        );
    }

    #[test]
    fn automatic_candidate_does_not_run_from_detector_name_only() {
        let candidate = available("dev", 95, SelectionPolicy::Automatic);
        let result = resolve(vec![candidate], &["node".to_owned()], 1, false);
        assert_eq!(result.status, ResolutionStatus::Ambiguous);
        assert_eq!(
            result.candidates[0].query.terms[0].field,
            SearchField::Detector
        );
    }

    #[test]
    fn declared_exact_scope_can_select_a_clear_automatic_candidate() {
        let mut candidate = available("dev", 95, SelectionPolicy::Automatic);
        candidate.search.scopes.push("web".to_owned());
        let result = resolve(vec![candidate], &["web".to_owned()], 1, false);
        assert_eq!(result.status, ResolutionStatus::Resolved);
        assert_eq!(result.reason, ResolutionReason::ScopedStructuralWinner);
    }

    #[test]
    fn chaos_two_does_not_relax_the_automatic_identity_gate() {
        let result = resolve(
            vec![available("abcdefghij", 80, SelectionPolicy::ExplicitHint)],
            &["abcdzzzzij".to_owned()],
            2,
            false,
        );
        assert_eq!(result.status, ResolutionStatus::Ambiguous);
        assert!(!result.candidates[0].query.terms.is_empty());
        assert!(result.candidates[0].query.best_identity_quality < 860);
    }
}
