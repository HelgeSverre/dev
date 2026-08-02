use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::intent::{Intent, Invocation};
use crate::query::normalize_query;
use crate::scan::RootInfo;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CacheKey {
    #[serde(with = "super::serde_os::path")]
    pub physical_anchor: PathBuf,
    pub intent: Intent,
    pub query: QueryCacheKey,
    pub chaos: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryCacheKey {
    Unhinted,
    Hinted(String),
}

#[must_use]
pub fn cache_key(invocation: &Invocation, roots: &RootInfo) -> CacheKey {
    let physical_anchor = roots
        .physical_anchor
        .clone()
        .unwrap_or_else(|| invocation.target.path().to_path_buf());
    let query = if invocation.hints.is_empty() {
        QueryCacheKey::Unhinted
    } else {
        let mut hasher = blake3::Hasher::new();
        for term in normalize_query(&invocation.hints) {
            let value = if term.normalized.compact.is_empty() {
                term.normalized.original.to_lowercase()
            } else {
                term.normalized.compact
            };
            hasher.update(&(value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
        QueryCacheKey::Hinted(hasher.finalize().to_hex().to_string())
    };
    CacheKey {
        physical_anchor,
        intent: invocation.intent,
        query,
        chaos: invocation.chaos,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::intent::{Intent, Target};

    use super::*;

    fn roots() -> RootInfo {
        RootInfo {
            logical_anchor: PathBuf::from("/project"),
            physical_anchor: Some(PathBuf::from("/physical/project")),
            package_root: Some(PathBuf::from("/project")),
            workspace_root: None,
            repository_root: None,
            scan_root: PathBuf::from("/project"),
        }
    }

    fn invocation(hints: &[&str]) -> Invocation {
        Invocation {
            intent: Intent::Run,
            target: Target::Directory(PathBuf::from("/project")),
            hints: hints.iter().map(|hint| (*hint).to_owned()).collect(),
            passthrough: Vec::new(),
            chaos: 1,
        }
    }

    #[test]
    fn query_hash_is_order_independent_and_deduplicated() {
        let first = cache_key(&invocation(&["participant", "sync"]), &roots());
        let second = cache_key(
            &invocation(&["SYNC", "participant", "participant"]),
            &roots(),
        );
        assert_eq!(first, second);
    }

    #[test]
    fn filler_only_query_does_not_reuse_the_unhinted_key() {
        let unhinted = cache_key(&invocation(&[]), &roots());
        let filler = cache_key(&invocation(&["please"]), &roots());
        assert_ne!(unhinted.query, filler.query);
        assert!(matches!(unhinted.query, QueryCacheKey::Unhinted));
        assert!(matches!(filler.query, QueryCacheKey::Hinted(_)));
    }

    #[test]
    fn chaos_levels_never_share_cache_entries() {
        let first = invocation(&["participant"]);
        let mut second = first.clone();
        second.chaos = 2;
        assert_ne!(cache_key(&first, &roots()), cache_key(&second, &roots()));
    }
}
