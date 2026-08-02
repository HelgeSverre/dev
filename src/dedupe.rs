use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;

use crate::candidate::{Candidate, PassthroughStyle, SearchDocument};
use crate::intent::{Intent, Target};
use crate::score;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct CommandKey {
    intent: Intent,
    program: OsString,
    args: Vec<OsString>,
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
    passthrough: PassthroughStyle,
}

impl From<&Candidate> for CommandKey {
    fn from(candidate: &Candidate) -> Self {
        Self {
            intent: candidate.intent,
            program: candidate.program.clone(),
            args: candidate.args.clone(),
            cwd: candidate.cwd.clone(),
            env: candidate
                .env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            passthrough: candidate.passthrough,
        }
    }
}

/// Merge execution-equivalent candidates and recompute their scores.
#[must_use]
pub fn deduplicate(candidates: Vec<Candidate>, target: &Target) -> Vec<Candidate> {
    let mut merged = BTreeMap::<CommandKey, Candidate>::new();
    for candidate in candidates {
        let key = CommandKey::from(&candidate);
        match merged.get_mut(&key) {
            Some(existing) => merge_into(existing, candidate),
            None => {
                merged.insert(key, candidate);
            }
        }
    }

    let mut candidates = merged
        .into_values()
        .map(|mut candidate| {
            score::finalize(&mut candidate, target);
            candidate
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.id.cmp(&right.id));
    candidates
}

fn merge_into(existing: &mut Candidate, incoming: Candidate) {
    existing.base_points = existing.base_points.max(incoming.base_points);
    existing.selection = existing.selection.strictest(incoming.selection);
    existing.evidence.extend(incoming.evidence);
    merge_search(&mut existing.search, incoming.search);

    if detector_specificity(incoming.detector) > detector_specificity(existing.detector) {
        existing.detector = incoming.detector;
        existing.action_key = incoming.action_key;
        existing.action_name = incoming.action_name;
        existing.label = incoming.label;
        existing.description = incoming.description;
        existing.lifecycle = incoming.lifecycle;
        existing.origin = incoming.origin;
    }
}

fn merge_search(existing: &mut SearchDocument, incoming: SearchDocument) {
    merge_values(&mut existing.identities, incoming.identities);
    merge_paths(&mut existing.target_paths, incoming.target_paths);
    merge_values(&mut existing.scopes, incoming.scopes);
    merge_values(&mut existing.tags, incoming.tags);
    merge_values(&mut existing.text, incoming.text);
}

fn merge_values(existing: &mut Vec<String>, incoming: Vec<String>) {
    let values = std::mem::take(existing)
        .into_iter()
        .chain(incoming)
        .collect::<BTreeSet<_>>();
    existing.extend(values);
}

fn merge_paths(existing: &mut Vec<PathBuf>, incoming: Vec<PathBuf>) {
    let values = std::mem::take(existing)
        .into_iter()
        .chain(incoming)
        .collect::<BTreeSet<_>>();
    existing.extend(values);
}

fn detector_specificity(detector: &str) -> u8 {
    match detector {
        "vite" | "next" | "artisan" => 3,
        "node" | "composer" | "cargo" | "go" | "zig" | "swift" | "dart" | "flutter" => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::candidate::{Availability, SelectionPolicy};
    use crate::intent::Intent;

    use super::*;

    fn candidate(detector: &'static str, points: i32) -> Candidate {
        let mut candidate = Candidate::new(
            format!("{detector}:run"),
            detector,
            Intent::Run,
            "run",
            "true",
            Vec::new(),
            PathBuf::from("/tmp"),
            points,
            SelectionPolicy::Automatic,
        );
        candidate.availability = Availability::Available {
            resolved_program: PathBuf::from("/usr/bin/true"),
        };
        candidate.env = BTreeMap::new();
        candidate
    }

    #[test]
    fn dedupe_is_idempotent() {
        let target = Target::Directory(PathBuf::from("/tmp"));
        let once = deduplicate(vec![candidate("node", 80), candidate("vite", 90)], &target);
        let twice = deduplicate(once.clone(), &target);
        assert_eq!(once.len(), 1);
        assert_eq!(twice.len(), 1);
        assert_eq!(once[0].id, twice[0].id);
        assert_eq!(once[0].structural_points, twice[0].structural_points);
    }
}
