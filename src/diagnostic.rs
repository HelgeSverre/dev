use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Severity for a non-fatal discovery diagnostic.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// A detector or scanner problem retained for explainability.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub detector: &'static str,
    pub severity: Severity,
    pub message: String,
    pub source: Option<PathBuf>,
}

impl Diagnostic {
    #[must_use]
    pub fn warning(
        detector: &'static str,
        message: impl Into<String>,
        source: Option<PathBuf>,
    ) -> Self {
        Self {
            detector,
            severity: Severity::Warning,
            message: message.into(),
            source,
        }
    }
}
