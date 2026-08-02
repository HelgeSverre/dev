use std::cmp::Ordering;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::candidate::{Candidate, Points};

use super::normalize::{normalize, QueryTerm};

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchClass {
    Context,
    Scope,
    Identity,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchStrategy {
    ExactSegment,
    ExactCompact,
    Prefix,
    Substring,
    Acronym,
    Subsequence,
    JaroWinkler,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchField {
    ActionIdentity,
    TargetStem,
    TargetPath,
    Scope,
    WorkingDirectory,
    Detector,
    Tag,
    Argument,
    Program,
    Label,
    Text,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TermMatch {
    pub hint: String,
    pub candidate_value: String,
    pub field: SearchField,
    pub class: MatchClass,
    pub strategy: MatchStrategy,
    pub quality_millis: u16,
    pub points: Points,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryMatch {
    pub terms: Vec<TermMatch>,
    pub highest_class: Option<MatchClass>,
    pub meaningful_terms: u16,
    pub matched_meaningful_terms: u16,
    pub coverage_millis: u16,
    pub best_identity_quality: u16,
    pub identity_points: Points,
    pub scope_points: Points,
    pub context_points: Points,
    pub total_points: Points,
}

#[derive(Clone, Debug)]
struct Surface {
    value: String,
    field: SearchField,
    class: MatchClass,
    weight: i32,
}

#[must_use]
pub fn match_candidate(candidate: &Candidate, query: &[QueryTerm], chaos: u8) -> QueryMatch {
    let surfaces = surfaces(candidate);
    let mut output = QueryMatch {
        meaningful_terms: query.iter().filter(|term| !term.filler).count() as u16,
        ..QueryMatch::default()
    };
    for term in query {
        let best = surfaces
            .iter()
            .filter_map(|surface| term_match(term, surface, chaos))
            .max_by(compare_term_matches);
        if let Some(best) = best {
            if !term.filler || best.class == MatchClass::Identity {
                if !term.filler {
                    output.matched_meaningful_terms += 1;
                }
                output.highest_class = output.highest_class.max(Some(best.class));
                match best.class {
                    MatchClass::Identity => {
                        output.identity_points += best.points;
                        output.best_identity_quality =
                            output.best_identity_quality.max(best.quality_millis);
                    }
                    MatchClass::Scope => output.scope_points += best.points,
                    MatchClass::Context => output.context_points += best.points,
                }
                output.terms.push(best);
            }
        }
    }
    if output.meaningful_terms > 0 {
        output.coverage_millis = u16::try_from(
            u32::from(output.matched_meaningful_terms) * 1000 / u32::from(output.meaningful_terms),
        )
        .unwrap_or(1000);
    }
    output.total_points = output.identity_points + output.scope_points + output.context_points;
    output
}

fn surfaces(candidate: &Candidate) -> Vec<Surface> {
    let mut values = Vec::new();
    values.extend(
        candidate
            .search
            .identities
            .iter()
            .cloned()
            .map(|value| Surface {
                value,
                field: SearchField::ActionIdentity,
                class: MatchClass::Identity,
                weight: 100,
            }),
    );
    for path in &candidate.search.target_paths {
        if let Some(stem) = path.file_stem() {
            values.push(Surface {
                value: stem.to_string_lossy().into_owned(),
                field: SearchField::TargetStem,
                class: MatchClass::Identity,
                weight: 100,
            });
        }
        values.extend(path_segments(path).map(|value| Surface {
            value,
            field: SearchField::TargetPath,
            class: MatchClass::Identity,
            weight: 90,
        }));
    }
    values.extend(
        candidate
            .search
            .scopes
            .iter()
            .cloned()
            .map(|value| Surface {
                value,
                field: SearchField::Scope,
                class: MatchClass::Scope,
                weight: 75,
            }),
    );
    values.extend(path_segments(&candidate.cwd).map(|value| Surface {
        value,
        field: SearchField::WorkingDirectory,
        class: MatchClass::Scope,
        weight: 65,
    }));
    values.push(Surface {
        value: candidate.detector.to_owned(),
        field: SearchField::Detector,
        class: MatchClass::Scope,
        weight: 60,
    });
    values.extend(candidate.search.tags.iter().cloned().map(|value| Surface {
        value,
        field: SearchField::Tag,
        class: MatchClass::Scope,
        weight: 55,
    }));
    values.extend(candidate.args.iter().map(|value| Surface {
        value: value.to_string_lossy().into_owned(),
        field: SearchField::Argument,
        class: MatchClass::Context,
        weight: 40,
    }));
    values.push(Surface {
        value: candidate.program.to_string_lossy().into_owned(),
        field: SearchField::Program,
        class: MatchClass::Context,
        weight: 35,
    });
    values.push(Surface {
        value: candidate.label.clone(),
        field: SearchField::Label,
        class: MatchClass::Context,
        weight: 30,
    });
    values.extend(candidate.search.text.iter().cloned().map(|value| Surface {
        value,
        field: SearchField::Text,
        class: MatchClass::Context,
        weight: 15,
    }));
    values
}

fn path_segments(path: &Path) -> impl Iterator<Item = String> + '_ {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
}

fn term_match(term: &QueryTerm, surface: &Surface, chaos: u8) -> Option<TermMatch> {
    let target = normalize(&surface.value);
    let hint = &term.normalized;
    if hint.compact.is_empty() || target.compact.is_empty() {
        return None;
    }

    let (strategy, quality_millis) = if target.segments.contains(&hint.compact) {
        (MatchStrategy::ExactSegment, 1000)
    } else if target.compact == hint.compact {
        (MatchStrategy::ExactCompact, 990)
    } else if hint.compact.len() >= 3
        && target
            .segments
            .iter()
            .any(|segment| segment.starts_with(&hint.compact))
    {
        let quality = 900 + length_ratio(&hint.compact, &target.compact) / 10;
        (MatchStrategy::Prefix, quality.min(980))
    } else if hint.compact.len() >= 4 && target.compact.contains(&hint.compact) {
        (MatchStrategy::Substring, 880)
    } else if (2..=4).contains(&hint.compact.len())
        && target.segments.len() >= 2
        && acronym(&target.segments) == hint.compact
    {
        (MatchStrategy::Acronym, 920)
    } else if chaos > 0 && hint.compact.len() >= 3 {
        fuzzy_quality(&hint.compact, &target.compact, chaos)?
    } else {
        return None;
    };

    if !quality_allowed(hint.compact.len(), quality_millis, chaos) {
        return None;
    }
    Some(TermMatch {
        hint: hint.original.clone(),
        candidate_value: surface.value.clone(),
        field: surface.field,
        class: surface.class,
        strategy,
        quality_millis,
        points: surface.weight * i32::from(quality_millis) / 1000,
    })
}

fn fuzzy_quality(hint: &str, target: &str, chaos: u8) -> Option<(MatchStrategy, u16)> {
    let jaro = (strsim::jaro_winkler(hint, target) * 1000.0).round() as u16;
    let nucleo = nucleo_quality(hint, target).unwrap_or(0);
    let (strategy, quality) = if nucleo > jaro {
        (MatchStrategy::Subsequence, nucleo)
    } else {
        (MatchStrategy::JaroWinkler, jaro)
    };
    quality_allowed(hint.len(), quality, chaos).then_some((strategy, quality))
}

fn nucleo_quality(needle: &str, haystack: &str) -> Option<u16> {
    use nucleo_matcher::{Config, Matcher, Utf32Str};

    if needle.chars().count() > 128 || haystack.chars().count() > 512 {
        return None;
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut haystack_buffer = Vec::new();
    let mut needle_buffer = Vec::new();
    let score = matcher.fuzzy_match(
        Utf32Str::new(haystack, &mut haystack_buffer),
        Utf32Str::new(needle, &mut needle_buffer),
    )?;

    let mut baseline_matcher = Matcher::new(Config::DEFAULT);
    let mut baseline_haystack_buffer = Vec::new();
    let mut baseline_needle_buffer = Vec::new();
    let baseline = baseline_matcher.fuzzy_match(
        Utf32Str::new(needle, &mut baseline_haystack_buffer),
        Utf32Str::new(needle, &mut baseline_needle_buffer),
    )?;
    if baseline == 0 {
        return None;
    }
    let quality = u32::from(score)
        .saturating_mul(1000)
        .checked_div(u32::from(baseline))
        .unwrap_or(0)
        .min(1000);
    u16::try_from(quality).ok()
}

fn length_ratio(left: &str, right: &str) -> u16 {
    let minimum = left.chars().count().min(right.chars().count()) as u32;
    let maximum = left.chars().count().max(right.chars().count()) as u32;
    let ratio = minimum
        .saturating_mul(1000)
        .checked_div(maximum)
        .unwrap_or(0);
    u16::try_from(ratio).unwrap_or(1000)
}

fn quality_allowed(length: usize, quality: u16, chaos: u8) -> bool {
    match length {
        0 => false,
        1 => quality == 1000,
        2 => quality >= 920,
        3 => quality >= 900,
        4 | 5 => quality >= if chaos >= 2 { 780 } else { 820 },
        _ => quality >= if chaos >= 2 { 720 } else { 760 },
    }
}

fn acronym(segments: &[String]) -> String {
    segments
        .iter()
        .filter_map(|segment| segment.chars().next())
        .collect()
}

fn compare_term_matches(left: &TermMatch, right: &TermMatch) -> Ordering {
    left.class
        .cmp(&right.class)
        .then_with(|| left.quality_millis.cmp(&right.quality_millis))
        .then_with(|| left.points.cmp(&right.points))
        .then_with(|| right.candidate_value.cmp(&left.candidate_value))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::candidate::{Candidate, SearchDocument, SelectionPolicy};
    use crate::intent::Intent;

    use super::*;
    use crate::query::normalize_query;

    fn named_candidate(name: &str) -> Candidate {
        let mut candidate = Candidate::new(
            "node:script",
            "node",
            Intent::Run,
            name,
            "npm",
            Vec::new(),
            PathBuf::from("/tmp"),
            15,
            SelectionPolicy::ExplicitHint,
        );
        candidate.search = SearchDocument {
            identities: vec![name.to_owned()],
            ..SearchDocument::default()
        };
        candidate
    }

    #[test]
    fn typo_matches_identity_at_automatic_quality() {
        let candidate = named_candidate("wibble-wabble");
        let query = normalize_query(&["wibblewbale".to_owned()]);
        let matched = match_candidate(&candidate, &query, 1);
        assert_eq!(matched.highest_class, Some(MatchClass::Identity));
        assert!(matched.best_identity_quality >= 860);
    }

    #[test]
    fn short_tokens_do_not_match_substrings() {
        let candidate = named_candidate("cargoose");
        let query = normalize_query(&["go".to_owned()]);
        assert!(match_candidate(&candidate, &query, 2).terms.is_empty());
    }
}
