//! Core library for the `dev` command launcher.

pub mod candidate;
pub mod cli;
pub mod dedupe;
pub mod detect;
pub mod diagnostic;
pub mod exec;
pub mod intent;
pub mod path;
pub mod query;
pub mod resolve;
pub mod scan;
pub mod score;
pub mod ui;

pub use candidate::{Candidate, CandidateId};
pub use intent::{Intent, Invocation, Target};
