use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::registry::DetectorId;

/// Severity for a non-fatal discovery diagnostic.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// A detector or scanner problem retained for explainability.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub detector: DetectorId,
    pub severity: Severity,
    pub message: String,
    pub source: Option<PathBuf>,
}

impl Diagnostic {
    #[must_use]
    pub fn info(detector: DetectorId, message: impl Into<String>, source: Option<PathBuf>) -> Self {
        Self {
            detector,
            severity: Severity::Info,
            message: message.into(),
            source,
        }
    }

    #[must_use]
    pub fn warning(
        detector: DetectorId,
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

    #[must_use]
    pub fn error(
        detector: DetectorId,
        message: impl Into<String>,
        source: Option<PathBuf>,
    ) -> Self {
        Self {
            detector,
            severity: Severity::Error,
            message: message.into(),
            source,
        }
    }
}
