use std::ffi::OsStr;

use base64::Engine;
use serde::Serialize;

use crate::candidate::Availability;
use crate::diagnostic::Diagnostic;
use crate::intent::Invocation;
use crate::resolve::{RankedCandidate, Resolution};
use crate::scan::{FileIndex, RootInfo};

#[derive(Serialize)]
struct JsonOutput<'a> {
    schema_version: u32,
    invocation: JsonInvocation,
    scan: JsonScan,
    resolution: JsonResolution,
    candidates: Vec<JsonCandidate<'a>>,
    diagnostics: Vec<JsonDiagnostic<'a>>,
}

#[derive(Serialize)]
struct JsonInvocation {
    intent: String,
    target: String,
    hints: Vec<String>,
    chaos: u8,
}

#[derive(Serialize)]
struct JsonScan {
    package_root: Option<String>,
    workspace_root: Option<String>,
    scan_root: String,
    structural_entries: usize,
    target_entries: usize,
    truncated: bool,
    truncated_scopes: Vec<String>,
}

#[derive(Serialize)]
struct JsonResolution {
    status: crate::resolve::ResolutionStatus,
    selected_candidate_id: Option<String>,
    reason: crate::resolve::ResolutionReason,
}

#[derive(Serialize)]
struct JsonCandidate<'a> {
    id: &'a str,
    action_key: &'a str,
    detector: &'a str,
    intent: crate::intent::Intent,
    origin: crate::candidate::CandidateOrigin,
    policy: crate::candidate::SelectionPolicy,
    availability: JsonAvailability,
    program: JsonOsValue,
    args: Vec<JsonOsValue>,
    cwd: String,
    environment: Vec<JsonEnvironment>,
    passthrough: crate::candidate::PassthroughStyle,
    lifecycle: crate::candidate::Lifecycle,
    structural_rank: i32,
    query_rank: i32,
    structural_evidence: Vec<JsonEvidence<'a>>,
    query_evidence: &'a [crate::query::TermMatch],
    query_coverage_millis: u16,
    finalist: bool,
    label: &'a str,
    description: &'a str,
}

#[derive(Serialize)]
struct JsonEnvironment {
    key: JsonOsValue,
    value: JsonOsValue,
}

#[derive(Serialize)]
struct JsonOsValue {
    display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_base64: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum JsonAvailability {
    Available { resolved_program: String },
    MissingProgram { program: JsonOsValue },
    UnsupportedHost { reason: String },
}

#[derive(Serialize)]
struct JsonDiagnostic<'a> {
    detector: &'a str,
    severity: crate::diagnostic::Severity,
    message: &'a str,
    source: Option<String>,
}

#[derive(Serialize)]
struct JsonEvidence<'a> {
    kind: crate::candidate::EvidenceKind,
    reason: &'a str,
    points: i32,
    source: Option<String>,
}

pub fn render(
    invocation: &Invocation,
    roots: &RootInfo,
    index: &FileIndex,
    resolution: &Resolution,
    diagnostics: &[Diagnostic],
) -> Result<String, serde_json::Error> {
    let output = JsonOutput {
        schema_version: 1,
        invocation: JsonInvocation {
            intent: invocation.intent.to_string(),
            target: invocation.target.path().to_string_lossy().into_owned(),
            hints: invocation.hints.clone(),
            chaos: invocation.chaos,
        },
        scan: JsonScan {
            package_root: roots
                .package_root
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            workspace_root: roots
                .workspace_root
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            scan_root: roots.scan_root.to_string_lossy().into_owned(),
            structural_entries: index.structural.len(),
            target_entries: index.targets.len(),
            truncated: !index.truncated.is_empty(),
            truncated_scopes: index
                .truncated
                .iter()
                .map(|truncation| truncation.scope.to_string_lossy().into_owned())
                .collect(),
        },
        resolution: JsonResolution {
            status: resolution.status,
            selected_candidate_id: resolution
                .selected_candidate()
                .map(|candidate| candidate.id.as_str().to_owned()),
            reason: resolution.reason,
        },
        candidates: resolution
            .candidates
            .iter()
            .map(JsonCandidate::from)
            .collect(),
        diagnostics: diagnostics
            .iter()
            .map(|diagnostic| JsonDiagnostic {
                detector: diagnostic.detector,
                severity: diagnostic.severity,
                message: &diagnostic.message,
                source: diagnostic
                    .source
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&output)
}

impl<'a> From<&'a RankedCandidate> for JsonCandidate<'a> {
    fn from(ranked: &'a RankedCandidate) -> Self {
        let candidate = &ranked.candidate;
        Self {
            id: candidate.id.as_str(),
            action_key: &candidate.action_key,
            detector: candidate.detector,
            intent: candidate.intent,
            origin: candidate.origin,
            policy: candidate.selection,
            availability: JsonAvailability::from(&candidate.availability),
            program: JsonOsValue::from(candidate.program.as_os_str()),
            args: candidate
                .args
                .iter()
                .map(|value| JsonOsValue::from(value.as_os_str()))
                .collect(),
            cwd: candidate.cwd.to_string_lossy().into_owned(),
            environment: candidate
                .env
                .iter()
                .map(|(key, value)| JsonEnvironment {
                    key: JsonOsValue::from(key.as_os_str()),
                    value: JsonOsValue::from(value.as_os_str()),
                })
                .collect(),
            passthrough: candidate.passthrough,
            lifecycle: candidate.lifecycle,
            structural_rank: candidate.structural_points,
            query_rank: ranked.query.total_points,
            structural_evidence: candidate
                .evidence
                .iter()
                .map(|evidence| JsonEvidence {
                    kind: evidence.kind,
                    reason: &evidence.reason,
                    points: evidence.points,
                    source: evidence
                        .source
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                })
                .collect(),
            query_evidence: &ranked.query.terms,
            query_coverage_millis: ranked.query.coverage_millis,
            finalist: ranked.finalist,
            label: &candidate.label,
            description: &candidate.description,
        }
    }
}

impl From<&Availability> for JsonAvailability {
    fn from(availability: &Availability) -> Self {
        match availability {
            Availability::Available { resolved_program } => Self::Available {
                resolved_program: resolved_program.to_string_lossy().into_owned(),
            },
            Availability::MissingProgram { program } => Self::MissingProgram {
                program: JsonOsValue::from(program.as_os_str()),
            },
            Availability::UnsupportedHost { reason } => Self::UnsupportedHost {
                reason: reason.clone(),
            },
        }
    }
}

impl From<&OsStr> for JsonOsValue {
    fn from(value: &OsStr) -> Self {
        Self {
            display: value.to_string_lossy().into_owned(),
            bytes_base64: os_bytes(value),
        }
    }
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    value
        .to_str()
        .is_none()
        .then(|| base64::engine::general_purpose::STANDARD.encode(value.as_bytes()))
}

#[cfg(not(unix))]
fn os_bytes(_value: &OsStr) -> Option<String> {
    None
}
