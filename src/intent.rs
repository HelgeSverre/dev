use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The semantic action requested by the user.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    Run,
    Build,
    Test,
}

impl fmt::Display for Intent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Run => "run",
            Self::Build => "build",
            Self::Test => "test",
        })
    }
}

impl FromStr for Intent {
    type Err = ParseIntentError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "run" => Ok(Self::Run),
            "build" => Ok(Self::Build),
            "test" => Ok(Self::Test),
            _ => Err(ParseIntentError(value.to_owned())),
        }
    }
}

/// Error returned when text is not a supported intent.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("unsupported intent `{0}`; expected run, build, or test")]
pub struct ParseIntentError(String);

/// A resolved filesystem target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    Directory(PathBuf),
    File(PathBuf),
}

impl Target {
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::Directory(path) | Self::File(path) => path,
        }
    }

    #[must_use]
    pub fn anchor_directory(&self) -> &std::path::Path {
        match self {
            Self::Directory(path) => path,
            Self::File(path) => path.parent().unwrap_or(path),
        }
    }
}

/// Fully parsed invocation passed into discovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invocation {
    pub intent: Intent,
    pub target: Target,
    pub hints: Vec<String>,
    pub passthrough: Vec<OsString>,
    pub chaos: u8,
}
