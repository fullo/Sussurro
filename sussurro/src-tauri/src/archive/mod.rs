//! The archive (plan §4.4): every non-dictation transcript becomes a folder
//! of plain files under `<Documents>/Sussurro`, with a derived SQLite FTS5
//! search index in the app data dir.
//!
//! - [`paths`]: location, folder naming, id validation and confinement
//! - [`types`]: frontmatter and `segments.json` data model
//! - [`frontmatter`]: YAML frontmatter read/write
//! - [`render`]: `transcript.md` rendering
//! - [`store`]: create / read / list / update / delete items
//! - [`index`]: search index (rebuildable)

pub mod frontmatter;
pub mod index;
pub mod paths;
pub mod render;
pub mod store;
pub mod types;

pub use index::{rebuild_index, with_index, Index, SearchFilters};
pub use paths::resolve_archive_dir;
pub use store::{
    create_item, delete_item, list_items, read_item, save_segments, update_meta, Item, ItemSummary,
};
pub use types::{
    Channel, DocSpeaker, ItemMeta, ItemType, Participant, Segment, SegmentsFile, Word,
};

/// File name of the search index inside the app data dir.
pub const INDEX_FILE: &str = "archive-index.sqlite";
