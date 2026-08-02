pub mod matcher;
pub mod normalize;
pub mod rank;

pub use matcher::{match_candidate, MatchClass, MatchStrategy, QueryMatch, SearchField, TermMatch};
pub use normalize::{normalize, normalize_query, NormalizedText, QueryTerm};
