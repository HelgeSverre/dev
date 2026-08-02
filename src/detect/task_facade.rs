use crate::candidate::{Lifecycle, SelectionPolicy};
use crate::intent::Intent;

pub(super) fn policy<'a>(
    intent: Intent,
    names: impl IntoIterator<Item = &'a str>,
    is_default: bool,
) -> Option<(SelectionPolicy, i32)> {
    let names = names.into_iter().collect::<Vec<_>>();
    if let Some(points) = names
        .iter()
        .filter_map(|name| canonical_points(intent, name))
        .max()
    {
        return Some((SelectionPolicy::Automatic, points));
    }
    if names
        .iter()
        .any(|name| compound_name_matches_intent(intent, name))
    {
        return Some((SelectionPolicy::ExplicitHint, 15));
    }
    match intent {
        Intent::Run => {
            let canonical_other = names.iter().any(|name| {
                canonical_points(Intent::Build, name).is_some()
                    || canonical_points(Intent::Test, name).is_some()
            });
            if is_default && !canonical_other {
                Some((SelectionPolicy::Automatic, 85))
            } else {
                Some((SelectionPolicy::ExplicitHint, 15))
            }
        }
        Intent::Build | Intent::Test => None,
    }
}

fn compound_name_matches_intent(intent: Intent, name: &str) -> bool {
    let mut segments = name
        .split(['.', ':', '-', '_'])
        .filter(|segment| !segment.is_empty());
    match intent {
        Intent::Run => false,
        Intent::Build => segments.any(|segment| matches!(segment, "build" | "compile" | "bundle")),
        Intent::Test => segments.any(|segment| matches!(segment, "test" | "check" | "verify")),
    }
}

pub(super) fn lifecycle<'a>(intent: Intent, names: impl IntoIterator<Item = &'a str>) -> Lifecycle {
    if intent == Intent::Run
        && names
            .into_iter()
            .any(|name| matches!(name, "dev" | "start" | "serve" | "watch"))
    {
        Lifecycle::LongRunning
    } else {
        Lifecycle::Finite
    }
}

fn canonical_points(intent: Intent, name: &str) -> Option<i32> {
    match intent {
        Intent::Run => match name {
            "dev" => Some(95),
            "run" | "start" => Some(90),
            "serve" | "watch" => Some(75),
            _ => None,
        },
        Intent::Build => match name {
            "build" => Some(95),
            "all" => Some(85),
            "compile" | "bundle" => Some(75),
            _ => None,
        },
        Intent::Test => match name {
            "test" => Some(95),
            "check" | "verify" => Some(75),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_facade_names_are_intent_specific() {
        assert_eq!(
            policy(Intent::Test, ["test"], false),
            Some((SelectionPolicy::Automatic, 95))
        );
        assert_eq!(policy(Intent::Build, ["test"], false), None);
        assert_eq!(
            policy(Intent::Run, ["custom"], true),
            Some((SelectionPolicy::Automatic, 85))
        );
        assert_eq!(
            policy(Intent::Run, ["build"], true),
            Some((SelectionPolicy::ExplicitHint, 15))
        );
        assert_eq!(
            policy(Intent::Build, ["build-all"], false),
            Some((SelectionPolicy::ExplicitHint, 15))
        );
        assert_eq!(
            policy(Intent::Test, ["test:integration"], false),
            Some((SelectionPolicy::ExplicitHint, 15))
        );
        assert_eq!(policy(Intent::Build, ["test-all"], false), None);
    }
}
