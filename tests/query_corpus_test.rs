use std::path::PathBuf;

use dev_launcher::candidate::{
    Availability, Candidate, Evidence, EvidenceKind, SearchDocument, SelectionPolicy,
};
use dev_launcher::detect::CandidateBuilder;
use dev_launcher::intent::Intent;
use dev_launcher::query::{
    match_candidate, normalize_query, MatchClass, MatchStrategy, QueryMatch,
};
use dev_launcher::registry::source_by_name;
use dev_launcher::resolve::{resolve, ResolutionStatus};

fn candidate(
    key: &str,
    detector: &'static str,
    identity: &str,
    policy: SelectionPolicy,
) -> Candidate {
    let (_, source) = source_by_name(detector)
        .unwrap_or_else(|| panic!("query fixture uses registered source `{detector}`"));
    CandidateBuilder::tool_default(source.id, Intent::Run, PathBuf::from("/fixture"), identity)
        .action_key(key)
        .program_path("tool")
        .args(Vec::<std::ffi::OsString>::new())
        .cwd(PathBuf::from("/fixture"))
        .selection(policy)
        .base_points(80)
        .label(identity)
        .description("query corpus fixture")
        .evidence(Evidence {
            kind: EvidenceKind::Rule,
            reason: "query corpus fixture".to_owned(),
            points: 0,
            source: None,
        })
        .search(SearchDocument {
            identities: vec![identity.to_owned()],
            ..SearchDocument::default()
        })
        .availability(Availability::Available {
            resolved_program: PathBuf::from("/fixture/tool"),
        })
        .build()
        .expect("query corpus fixture is complete")
}

fn matched(candidate: &Candidate, hints: &[&str], chaos: u8) -> QueryMatch {
    let hints = hints
        .iter()
        .map(|hint| (*hint).to_owned())
        .collect::<Vec<_>>();
    match_candidate(candidate, &normalize_query(&hints), chaos)
}

#[test]
fn identity_drift_and_acronyms_have_inspectable_evidence() {
    let wibble = candidate(
        "query:wibble-wabble",
        "node",
        "wibble-wabble",
        SelectionPolicy::ExplicitHint,
    );
    for hint in ["wibble_wabble", "wibble.wabble", "WibbleWabble"] {
        let query = matched(&wibble, &[hint], 1);
        assert_eq!(query.coverage_millis, 1000, "hint {hint}");
        assert_eq!(query.highest_class, Some(MatchClass::Identity));
        assert_eq!(query.terms[0].strategy, MatchStrategy::ExactCompact);
    }

    let typo = matched(&wibble, &["wibblewbale"], 1);
    assert_eq!(typo.highest_class, Some(MatchClass::Identity));
    assert!(typo.best_identity_quality >= 860);

    let prefix = candidate(
        "query:wibbleton",
        "node",
        "wibbleton",
        SelectionPolicy::ExplicitHint,
    );
    assert_eq!(
        matched(&prefix, &["wibble"], 0).terms[0].strategy,
        MatchStrategy::Prefix
    );
    assert_eq!(
        matched(&wibble, &["ww"], 0).terms[0].strategy,
        MatchStrategy::Acronym
    );

    let participant = candidate(
        "query:participant-sync",
        "artisan",
        "participant-sync",
        SelectionPolicy::ExplicitHint,
    );
    assert_eq!(
        matched(&participant, &["ps"], 0).terms[0].strategy,
        MatchStrategy::Acronym
    );
}

#[test]
fn scopes_combined_hints_and_deep_targets_preserve_coverage() {
    let mut participant = candidate(
        "query:participant-sync",
        "artisan",
        "participant-sync",
        SelectionPolicy::ExplicitHint,
    );
    participant.search.scopes.push("laravel".to_owned());
    participant.search.target_paths.push(PathBuf::from(
        "tests/integration/deep/participant_sync_test.php",
    ));
    let scoped = matched(&participant, &["laravel"], 1);
    assert_eq!(scoped.highest_class, Some(MatchClass::Scope));
    assert_eq!(scoped.coverage_millis, 1000);

    let combined = matched(&participant, &["laravel", "participant"], 1);
    assert_eq!(combined.highest_class, Some(MatchClass::Identity));
    assert_eq!(combined.matched_meaningful_terms, 2);
    assert_eq!(combined.coverage_millis, 1000);
    assert!(combined
        .terms
        .iter()
        .any(|term| term.class == MatchClass::Scope));
    assert!(combined
        .terms
        .iter()
        .any(|term| term.class == MatchClass::Identity));

    let deep = matched(&participant, &["participant_sync_test"], 1);
    assert_eq!(deep.highest_class, Some(MatchClass::Identity));

    for (detector, scope) in [("cargo", "rust"), ("vite", "vite")] {
        let mut scoped = candidate(
            &format!("query:{detector}"),
            detector,
            "serve",
            SelectionPolicy::Automatic,
        );
        scoped.search.tags.push(scope.to_owned());
        assert_eq!(
            matched(&scoped, &[scope], 0).highest_class,
            Some(MatchClass::Scope)
        );
    }
}

#[test]
fn duplicate_action_names_are_disambiguated_by_member_scope() {
    let mut api = candidate(
        "query:api:test",
        "node",
        "participant-test",
        SelectionPolicy::Automatic,
    );
    api.search.scopes.push("api".to_owned());
    let mut web = candidate(
        "query:web:test",
        "node",
        "participant-test",
        SelectionPolicy::Automatic,
    );
    web.search.scopes.push("web".to_owned());

    let result = resolve(
        vec![web, api],
        &["api".to_owned(), "participant".to_owned()],
        1,
        false,
    );
    assert_eq!(result.status, ResolutionStatus::Resolved);
    assert_eq!(
        result
            .selected_candidate()
            .map(|candidate| candidate.action_key.as_str()),
        Some("query:api:test")
    );
    assert_eq!(
        result
            .candidates
            .iter()
            .filter(|candidate| candidate.finalist)
            .count(),
        1
    );
    assert_eq!(result.candidates[0].query.coverage_millis, 1000);
}

#[test]
fn negative_queries_never_turn_context_into_execution() {
    let unrelated = candidate(
        "query:unrelated",
        "node",
        "abcdefgh",
        SelectionPolicy::Automatic,
    );
    assert!(matched(&unrelated, &["ijklmnop"], 2).terms.is_empty());
    assert!(matched(&unrelated, &["edcba"], 2).terms.is_empty());

    for (identity, hint) in [
        ("cargoose", "go"),
        ("javascript", "js"),
        ("rustacean", "rs"),
    ] {
        let short = candidate(
            &format!("query:{identity}"),
            "shell",
            identity,
            SelectionPolicy::Automatic,
        );
        assert!(matched(&short, &[hint], 2).terms.is_empty());
    }

    let filler = matched(&unrelated, &["whatever", "thing", "in"], 2);
    assert_eq!(filler.meaningful_terms, 0);
    assert_eq!(filler.matched_meaningful_terms, 0);

    let unmatched = resolve(
        vec![unrelated],
        &["purple".to_owned(), "lasagna".to_owned()],
        2,
        false,
    );
    assert_eq!(unmatched.status, ResolutionStatus::HintNoMatch);
    assert!(unmatched
        .candidates
        .iter()
        .all(|candidate| !candidate.finalist));
}

#[test]
fn broad_scopes_and_chaos_two_stay_behind_auto_run_gates() {
    let mut dev = candidate("query:vite:dev", "vite", "dev", SelectionPolicy::Automatic);
    dev.search.scopes.push("vite".to_owned());
    let mut preview = candidate(
        "query:vite:preview",
        "vite",
        "preview",
        SelectionPolicy::Automatic,
    );
    preview.search.scopes.push("vite".to_owned());
    let broad = resolve(vec![dev, preview], &["vite".to_owned()], 2, false);
    assert_eq!(broad.status, ResolutionStatus::Ambiguous);
    assert_eq!(
        broad
            .candidates
            .iter()
            .filter(|candidate| candidate.finalist)
            .count(),
        2
    );

    let mut explicit = candidate(
        "query:laravel:worker",
        "artisan",
        "worker",
        SelectionPolicy::ExplicitHint,
    );
    explicit.search.scopes.push("laravel".to_owned());
    let scope_only = resolve(vec![explicit], &["laravel".to_owned()], 2, false);
    assert_eq!(scope_only.status, ResolutionStatus::Ambiguous);
    assert_eq!(
        scope_only.candidates[0].query.highest_class,
        Some(MatchClass::Scope)
    );

    let weak = resolve(
        vec![candidate(
            "query:weak",
            "node",
            "abcdefghij",
            SelectionPolicy::ExplicitHint,
        )],
        &["abcdzzzzij".to_owned()],
        2,
        false,
    );
    assert_eq!(weak.status, ResolutionStatus::Ambiguous);
    assert!(!weak.candidates[0].query.terms.is_empty());
    assert!(weak.candidates[0].query.best_identity_quality < 860);
}
