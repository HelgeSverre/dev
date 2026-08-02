use std::cmp::Ordering;

use crate::candidate::Candidate;

use super::QueryMatch;

pub fn compare_hinted(
    left: (&Candidate, &QueryMatch),
    right: (&Candidate, &QueryMatch),
) -> Ordering {
    right
        .1
        .highest_class
        .cmp(&left.1.highest_class)
        .then_with(|| {
            right
                .1
                .best_identity_quality
                .cmp(&left.1.best_identity_quality)
        })
        .then_with(|| right.1.identity_points.cmp(&left.1.identity_points))
        .then_with(|| right.1.coverage_millis.cmp(&left.1.coverage_millis))
        .then_with(|| right.1.total_points.cmp(&left.1.total_points))
        .then_with(|| right.1.scope_points.cmp(&left.1.scope_points))
        .then_with(|| right.0.structural_points.cmp(&left.0.structural_points))
        .then_with(|| left.0.anchor_distance.cmp(&right.0.anchor_distance))
        .then_with(|| left.0.action_key.cmp(&right.0.action_key))
        .then_with(|| left.0.id.cmp(&right.0.id))
}
