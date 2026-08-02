use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::intent::Intent;
use crate::registry::{CandidateSourceId, DetectorId};

pub type Points = i32;

/// Stable identity of the exact executable command semantics.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CandidateId(String);

impl CandidateId {
    #[must_use]
    pub fn from_command(candidate: &Candidate) -> Self {
        let mut hasher = blake3::Hasher::new();
        hash_text(&mut hasher, &candidate.intent.to_string());
        hash_os(&mut hasher, &candidate.program);
        for argument in &candidate.args {
            hash_os(&mut hasher, argument);
        }
        hash_os(&mut hasher, candidate.cwd.as_os_str());
        for (key, value) in &candidate.env {
            hash_os(&mut hasher, key);
            hash_os(&mut hasher, value);
        }
        hash_text(&mut hasher, &format!("{:?}", candidate.passthrough));
        Self(hasher.finalize().to_hex().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CandidateId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("CandidateId").field(&self.0).finish()
    }
}

fn hash_text(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(unix)]
fn hash_os(hasher: &mut blake3::Hasher, value: &OsStr) {
    use std::os::unix::ffi::OsStrExt;
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
    hash_text(hasher, &value.to_string_lossy());
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionPolicy {
    Automatic,
    ExplicitHint,
    Confirm,
}

impl SelectionPolicy {
    #[must_use]
    pub fn strictest(self, other: Self) -> Self {
        match (self, other) {
            (Self::Confirm, _) | (_, Self::Confirm) => Self::Confirm,
            (Self::ExplicitHint, _) | (_, Self::ExplicitHint) => Self::ExplicitHint,
            (Self::Automatic, Self::Automatic) => Self::Automatic,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateOrigin {
    Declared,
    Conventional,
    Synthetic,
}

/// Semantic layer of a command relative to the project interface.
#[derive(
    Copy, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CommandLayer {
    ProjectFacade,
    EcosystemTask,
    #[default]
    ToolDefault,
    DirectTarget,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Finite,
    LongRunning,
    MultiProcess,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Availability {
    Available { resolved_program: PathBuf },
    MissingProgram { program: OsString },
    UnsupportedHost { reason: String },
}

impl Availability {
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassthroughStyle {
    Append,
    DoubleDash,
    NpmRun,
    Custom,
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Manifest,
    Convention,
    Proximity,
    Availability,
    Rule,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub reason: String,
    pub points: Points,
    pub source: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchDocument {
    pub identities: Vec<String>,
    pub target_paths: Vec<PathBuf>,
    pub scopes: Vec<String>,
    pub tags: Vec<String>,
    pub text: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: CandidateId,
    pub action_key: String,
    pub detector: DetectorId,
    pub source: CandidateSourceId,
    pub intent: Intent,
    pub action_name: String,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Directory the candidate belongs to for proximity scoring.
    ///
    /// This differs from `cwd` for commands that execute from a workspace root
    /// while selecting a nested member through package-manager arguments.
    pub scope_root: PathBuf,
    pub env: BTreeMap<OsString, OsString>,
    pub passthrough: PassthroughStyle,
    pub lifecycle: Lifecycle,
    pub origin: CandidateOrigin,
    pub layer: CommandLayer,
    pub selection: SelectionPolicy,
    pub availability: Availability,
    pub base_points: Points,
    pub structural_points: Points,
    pub evidence: Vec<Evidence>,
    pub search: SearchDocument,
    pub label: String,
    pub description: String,
    pub anchor_distance: usize,
}

impl Candidate {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        action_key: impl Into<String>,
        detector: DetectorId,
        source: CandidateSourceId,
        intent: Intent,
        action_name: impl Into<String>,
        program: impl Into<OsString>,
        args: Vec<OsString>,
        cwd: PathBuf,
        base_points: Points,
        selection: SelectionPolicy,
    ) -> Self {
        let mut candidate = Self {
            id: CandidateId(String::new()),
            action_key: action_key.into(),
            detector,
            source,
            intent,
            action_name: action_name.into(),
            program: program.into(),
            args,
            scope_root: cwd.clone(),
            cwd,
            env: BTreeMap::new(),
            passthrough: PassthroughStyle::Append,
            lifecycle: Lifecycle::Finite,
            origin: CandidateOrigin::Declared,
            layer: CommandLayer::ToolDefault,
            selection,
            availability: Availability::MissingProgram {
                program: OsString::new(),
            },
            base_points,
            structural_points: base_points,
            evidence: Vec::new(),
            search: SearchDocument::default(),
            label: String::new(),
            description: String::new(),
            anchor_distance: usize::MAX,
        };
        candidate.id = CandidateId::from_command(&candidate);
        candidate
    }

    pub fn refresh_id(&mut self) {
        self.id = CandidateId::from_command(self);
    }

    #[must_use]
    pub fn command_with_passthrough(&self, passthrough: &[OsString]) -> Vec<OsString> {
        let mut args = self.args.clone();
        match self.passthrough {
            PassthroughStyle::Append | PassthroughStyle::Custom => {
                args.extend_from_slice(passthrough);
            }
            PassthroughStyle::DoubleDash | PassthroughStyle::NpmRun => {
                if !passthrough.is_empty() {
                    args.push(OsString::from("--"));
                    args.extend_from_slice(passthrough);
                }
            }
        }
        args
    }
}
