use std::ffi::{OsStr, OsString};
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
    #[serde(default)]
    pub environment_fingerprint: String,
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
        environment_fingerprint: environment_fingerprint(),
    }
}

fn environment_fingerprint() -> String {
    environment_fingerprint_with(|key| std::env::var_os(key))
}

fn environment_fingerprint_with(mut value: impl FnMut(&str) -> Option<OsString>) -> String {
    let keys = crate::registry::cache_environment();
    if keys.is_empty() {
        return String::new();
    }
    let mut hasher = blake3::Hasher::new();
    for key in keys {
        hasher.update(&(key.len() as u64).to_le_bytes());
        hasher.update(key.as_bytes());
        if let Some(value) = value(key) {
            hasher.update(&[1]);
            hash_os(&mut hasher, &value);
        } else {
            hasher.update(&[0]);
        }
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(unix)]
fn hash_os(hasher: &mut blake3::Hasher, value: &OsStr) {
    use std::os::unix::ffi::OsStrExt as _;

    let bytes = value.as_bytes();
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[cfg(windows)]
fn hash_os(hasher: &mut blake3::Hasher, value: &OsStr) {
    use std::os::windows::ffi::OsStrExt as _;

    let wide = value.encode_wide().collect::<Vec<_>>();
    hasher.update(&(wide.len() as u64).to_le_bytes());
    for unit in wide {
        hasher.update(&unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, windows)))]
fn hash_os(hasher: &mut blake3::Hasher, value: &OsStr) {
    let value = value.to_string_lossy();
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
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
            discovery_files: crate::scan::DiscoveryFiles::default(),
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

    #[test]
    fn registered_ambient_configuration_changes_cache_identity() {
        let development = environment_fingerprint_with(|key| {
            (key == "MISE_ENV").then(|| OsString::from("development"))
        });
        let production = environment_fingerprint_with(|key| {
            (key == "MISE_ENV").then(|| OsString::from("production"))
        });
        let missing = environment_fingerprint_with(|_| None);
        assert_ne!(development, production);
        assert_ne!(development, missing);
    }
}
