pub mod index;
pub mod manifest;
pub mod roots;
pub mod target_index;

pub use index::{FileIndex, IndexEntry, IndexedFileType, ScanOptions, Truncation};
pub use manifest::DiscoveryFiles;
pub use roots::{resolve_roots, RootInfo};
