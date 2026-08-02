mod key;
mod lock;
mod serde_os;
mod shape;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::candidate::{
    Candidate, CandidateId, CandidateOrigin, Lifecycle, PassthroughStyle, SelectionPolicy,
};
use crate::intent::{Invocation, Target};
use crate::scan::{FileIndex, RootInfo};

pub use key::{cache_key, CacheKey, QueryCacheKey};
pub use shape::ShapeSnapshot;

use serde_os::StoredOsString;

const CACHE_SCHEMA: u32 = 1;
const DETECTOR_SCHEMA: u32 = 3;
const MATCHER_SCHEMA: u32 = 1;
const MAX_ENTRIES: usize = 500;
const MAX_AGE: Duration = Duration::from_secs(90 * 24 * 60 * 60);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key: CacheKey,
    pub action_key: String,
    pub candidate_id: CandidateId,
    pub command_fingerprint: String,
    pub shape: ShapeSnapshot,
    pub cache_schema: u32,
    #[serde(default)]
    pub detector_schema: u32,
    pub matcher_schema: u32,
    pub chosen_at_millis: u64,
    pub last_used_at_millis: u64,
    command: StoredCommand,
}

impl CacheEntry {
    #[must_use]
    pub fn candidate(&self, target: &Target) -> Option<Candidate> {
        let mut candidate = self.command.to_candidate()?;
        crate::score::finalize(&mut candidate, target);
        (candidate.id == self.candidate_id && candidate.id.as_str() == self.command_fingerprint)
            .then_some(candidate)
    }

    #[must_use]
    pub fn is_shape_valid(&self) -> bool {
        self.cache_schema == CACHE_SCHEMA
            && self.detector_schema == DETECTOR_SCHEMA
            && self.matcher_schema == MATCHER_SCHEMA
            && self.shape.is_current()
    }

    #[must_use]
    pub fn needs_touch(&self) -> bool {
        now_millis().saturating_sub(self.last_used_at_millis)
            >= u64::try_from(Duration::from_secs(24 * 60 * 60).as_millis()).unwrap_or(u64::MAX)
    }

    #[must_use]
    pub fn age(&self) -> Duration {
        Duration::from_millis(now_millis().saturating_sub(self.chosen_at_millis))
    }
}

#[derive(Clone, Debug)]
pub enum CacheLookup {
    Missing,
    Valid(CacheEntry),
    Stale(CacheEntry),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ChoiceStore {
    schema_version: u32,
    entries: Vec<CacheEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredCommand {
    action_key: String,
    detector: String,
    intent: crate::intent::Intent,
    action_name: String,
    program: StoredOsString,
    args: Vec<StoredOsString>,
    #[serde(with = "serde_os::path")]
    cwd: PathBuf,
    env: Vec<StoredEnvironment>,
    passthrough: PassthroughStyle,
    lifecycle: Lifecycle,
    origin: CandidateOrigin,
    selection: SelectionPolicy,
    base_points: i32,
    label: String,
    description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredEnvironment {
    key: StoredOsString,
    value: StoredOsString,
}

impl StoredCommand {
    fn from_candidate(candidate: &Candidate) -> Self {
        Self {
            action_key: candidate.action_key.clone(),
            detector: candidate.detector.to_owned(),
            intent: candidate.intent,
            action_name: candidate.action_name.clone(),
            program: StoredOsString(candidate.program.clone()),
            args: candidate.args.iter().cloned().map(StoredOsString).collect(),
            cwd: candidate.cwd.clone(),
            env: candidate
                .env
                .iter()
                .map(|(key, value)| StoredEnvironment {
                    key: StoredOsString(key.clone()),
                    value: StoredOsString(value.clone()),
                })
                .collect(),
            passthrough: candidate.passthrough,
            lifecycle: candidate.lifecycle,
            origin: candidate.origin,
            selection: candidate.selection,
            base_points: candidate.base_points,
            label: candidate.label.clone(),
            description: candidate.description.clone(),
        }
    }

    fn to_candidate(&self) -> Option<Candidate> {
        let detector = detector_name(&self.detector)?;
        let mut candidate = Candidate::new(
            &self.action_key,
            detector,
            self.intent,
            &self.action_name,
            self.program.0.clone(),
            self.args
                .iter()
                .map(|argument| argument.0.clone())
                .collect(),
            self.cwd.clone(),
            self.base_points,
            self.selection,
        );
        candidate.env = self
            .env
            .iter()
            .map(|entry| (entry.key.0.clone(), entry.value.0.clone()))
            .collect();
        candidate.passthrough = self.passthrough;
        candidate.lifecycle = self.lifecycle;
        candidate.origin = self.origin;
        candidate.label.clone_from(&self.label);
        candidate.description.clone_from(&self.description);
        candidate.refresh_id();
        Some(candidate)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cannot determine the state directory")]
    NoStateDirectory,
    #[error("cache I/O failed for `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cache JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cache lock remained busy")]
    LockTimeout,
    #[error("manifest cache failed while recording project shape: {0}")]
    Manifest(#[from] crate::scan::manifest::ManifestError),
}

#[must_use]
pub fn lookup(invocation: &Invocation, roots: &RootInfo) -> CacheLookup {
    let key = cache_key(invocation, roots);
    let Ok(store) = load_store() else {
        return CacheLookup::Missing;
    };
    let Some(entry) = store.entries.into_iter().find(|entry| entry.key == key) else {
        return CacheLookup::Missing;
    };
    if entry.is_shape_valid() {
        CacheLookup::Valid(entry)
    } else {
        CacheLookup::Stale(entry)
    }
}

pub fn remember(
    invocation: &Invocation,
    roots: &RootInfo,
    index: &FileIndex,
    candidate: &Candidate,
) -> Result<(), CacheError> {
    let key = cache_key(invocation, roots);
    let command = StoredCommand::from_candidate(candidate);
    let now = now_millis();
    let entry = CacheEntry {
        key: key.clone(),
        action_key: candidate.action_key.clone(),
        candidate_id: candidate.id.clone(),
        command_fingerprint: candidate.id.as_str().to_owned(),
        shape: ShapeSnapshot::capture(roots, index, candidate, &invocation.target)?,
        cache_schema: CACHE_SCHEMA,
        detector_schema: DETECTOR_SCHEMA,
        matcher_schema: MATCHER_SCHEMA,
        chosen_at_millis: now,
        last_used_at_millis: now,
        command,
    };
    lock::update_store(|store| {
        store.entries.retain(|existing| existing.key != key);
        store.entries.push(entry);
        prune(store);
    })
}

pub fn refresh(
    invocation: &Invocation,
    roots: &RootInfo,
    index: &FileIndex,
    candidate: &Candidate,
) -> Result<(), CacheError> {
    let key = cache_key(invocation, roots);
    let command = StoredCommand::from_candidate(candidate);
    let shape = ShapeSnapshot::capture(roots, index, candidate, &invocation.target)?;
    let now = now_millis();
    lock::update_store(|store| {
        if let Some(entry) = store.entries.iter_mut().find(|entry| entry.key == key) {
            entry.action_key.clone_from(&candidate.action_key);
            entry.candidate_id.clone_from(&candidate.id);
            entry.command_fingerprint = candidate.id.as_str().to_owned();
            entry.shape = shape;
            entry.cache_schema = CACHE_SCHEMA;
            entry.detector_schema = DETECTOR_SCHEMA;
            entry.matcher_schema = MATCHER_SCHEMA;
            entry.last_used_at_millis = now;
            entry.command = command;
        }
        prune(store);
    })
}

pub fn touch(key: &CacheKey) -> Result<(), CacheError> {
    let now = now_millis();
    lock::update_store(|store| {
        if let Some(entry) = store.entries.iter_mut().find(|entry| &entry.key == key) {
            entry.last_used_at_millis = now;
        }
        prune(store);
    })
}

pub fn forget(invocation: &Invocation, roots: &RootInfo) -> Result<bool, CacheError> {
    let key = cache_key(invocation, roots);
    let mut removed = false;
    lock::update_store(|store| {
        let before = store.entries.len();
        store.entries.retain(|entry| entry.key != key);
        removed = store.entries.len() != before;
        prune(store);
    })?;
    Ok(removed)
}

pub fn list() -> Result<Vec<CacheEntry>, CacheError> {
    let mut entries = load_store()?.entries;
    entries.sort_by(|left, right| {
        right
            .last_used_at_millis
            .cmp(&left.last_used_at_millis)
            .then_with(|| left.key.cmp(&right.key))
    });
    Ok(entries)
}

pub fn clear() -> Result<usize, CacheError> {
    let mut count = 0;
    lock::update_store(|store| {
        count = store.entries.len();
        store.entries.clear();
    })?;
    Ok(count)
}

pub(crate) fn state_file() -> Result<PathBuf, CacheError> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })
        .ok_or(CacheError::NoStateDirectory)?;
    Ok(base.join("dev/choices.json"))
}

fn load_store() -> Result<ChoiceStore, CacheError> {
    let path = state_file()?;
    let contents = match std::fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ChoiceStore {
                schema_version: CACHE_SCHEMA,
                entries: Vec::new(),
            });
        }
        Err(source) => return Err(CacheError::Io { path, source }),
    };
    match serde_json::from_slice::<ChoiceStore>(&contents) {
        Ok(store) if store.schema_version == CACHE_SCHEMA => Ok(store),
        Ok(_) => Ok(ChoiceStore {
            schema_version: CACHE_SCHEMA,
            entries: Vec::new(),
        }),
        Err(error) => {
            eprintln!(
                "dev: warning: ignored corrupt cache `{}`: {error}",
                path.display()
            );
            let _ = lock::quarantine_corrupt(&path);
            Ok(ChoiceStore {
                schema_version: CACHE_SCHEMA,
                entries: Vec::new(),
            })
        }
    }
}

fn prune(store: &mut ChoiceStore) {
    store.schema_version = CACHE_SCHEMA;
    let cutoff =
        now_millis().saturating_sub(u64::try_from(MAX_AGE.as_millis()).unwrap_or(u64::MAX));
    store
        .entries
        .retain(|entry| entry.last_used_at_millis >= cutoff);
    store.entries.sort_by(|left, right| {
        right
            .last_used_at_millis
            .cmp(&left.last_used_at_millis)
            .then_with(|| left.key.cmp(&right.key))
    });
    store.entries.truncate(MAX_ENTRIES);
}

fn now_millis() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

fn detector_name(name: &str) -> Option<&'static str> {
    match name {
        "node" => Some("node"),
        "vite" => Some("vite"),
        "next" => Some("next"),
        "cargo" => Some("cargo"),
        "composer" => Some("composer"),
        "artisan" => Some("artisan"),
        "php-file" => Some("php-file"),
        "go" => Some("go"),
        "zig" => Some("zig"),
        "swift" => Some("swift"),
        "flutter" => Some("flutter"),
        "dart" => Some("dart"),
        "python-file" => Some("python-file"),
        "make" => Some("make"),
        "docker" => Some("docker"),
        "shell" => Some("shell"),
        _ => None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;
    use std::path::PathBuf;

    use crate::candidate::{Candidate, SelectionPolicy};
    use crate::intent::Intent;

    use super::StoredCommand;

    #[test]
    fn cached_commands_preserve_non_utf8_process_values() -> anyhow::Result<()> {
        let mut candidate = Candidate::new(
            "node:opaque",
            "node",
            Intent::Run,
            "opaque",
            OsString::from_vec(vec![b'n', 0x80, b'm']),
            vec![OsString::from_vec(vec![b'a', 0x81, b'g'])],
            PathBuf::from(OsString::from_vec(vec![b'/', b't', 0x82, b'p'])),
            50,
            SelectionPolicy::Automatic,
        );
        candidate.env = BTreeMap::from([(
            OsString::from_vec(vec![b'K', 0x83]),
            OsString::from_vec(vec![b'V', 0x84]),
        )]);
        candidate.refresh_id();

        let stored = StoredCommand::from_candidate(&candidate);
        let json = serde_json::to_vec(&stored)?;
        let decoded: StoredCommand = serde_json::from_slice(&json)?;
        let restored = decoded
            .to_candidate()
            .ok_or_else(|| anyhow::anyhow!("known detector must restore"))?;

        assert_eq!(restored.id, candidate.id);
        assert_eq!(restored.program, candidate.program);
        assert_eq!(restored.args, candidate.args);
        assert_eq!(restored.cwd, candidate.cwd);
        assert_eq!(restored.env, candidate.env);
        Ok(())
    }
}
