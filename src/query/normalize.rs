use std::collections::BTreeMap;

const FILLERS: &[&str] = &[
    "the", "a", "an", "in", "for", "from", "please", "thing", "whatever",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedText {
    pub original: String,
    pub segments: Vec<String>,
    pub compact: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryTerm {
    pub normalized: NormalizedText,
    pub filler: bool,
}

#[must_use]
pub fn normalize(value: &str) -> NormalizedText {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut previous_was_lower_or_digit = false;
    for character in value.chars() {
        let is_separator =
            character.is_whitespace() || matches!(character, '-' | '_' | '.' | ':' | '/' | '\\');
        if is_separator {
            push_segment(&mut segments, &mut current);
            previous_was_lower_or_digit = false;
            continue;
        }
        if character.is_uppercase() && previous_was_lower_or_digit {
            push_segment(&mut segments, &mut current);
        }
        current.extend(character.to_lowercase());
        previous_was_lower_or_digit = character.is_lowercase() || character.is_ascii_digit();
    }
    push_segment(&mut segments, &mut current);
    let compact = segments.concat();
    NormalizedText {
        original: value.to_owned(),
        segments,
        compact,
    }
}

fn push_segment(segments: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        segments.push(std::mem::take(current));
    }
}

#[must_use]
pub fn normalize_query(hints: &[String]) -> Vec<QueryTerm> {
    let mut unique = BTreeMap::new();
    for hint in hints {
        let normalized = normalize(hint);
        let key = if normalized.compact.is_empty() {
            hint.to_lowercase()
        } else {
            normalized.compact.clone()
        };
        unique.entry(key).or_insert_with(|| QueryTerm {
            filler: FILLERS.contains(&normalized.compact.as_str()),
            normalized,
        });
    }
    unique.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separators_and_camel_case_share_a_compact_form() {
        let expected = normalize("wibble-wabble").compact;
        assert_eq!(normalize("wibble_wabble").compact, expected);
        assert_eq!(normalize("WibbleWabble").compact, expected);
        assert_eq!(normalize("wibble.wabble").compact, expected);
    }

    #[test]
    fn duplicate_hints_count_once() {
        let terms = normalize_query(&["foo-bar".to_owned(), "FooBar".to_owned()]);
        assert_eq!(terms.len(), 1);
    }
}
