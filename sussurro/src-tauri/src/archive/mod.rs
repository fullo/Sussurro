//! The archive (plan §4.4): every non-dictation transcript becomes a folder
//! of plain files under `<Documents>/Sussurro`, with a derived SQLite FTS5
//! search index in the app data dir.
//!
//! - [`paths`]: location, folder naming, id validation and confinement
//! - [`types`]: frontmatter and `segments.json` data model
//! - [`companion`]: recipe output next to the transcript (`document.md`, #120)
//! - [`export`]: `.md`/`.txt`/`.srt`/`.vtt` exports and `transcript.srt` (#133)
//! - [`external`]: what was sent to an external LLM host, per item (#122)
//! - [`frontmatter`]: YAML frontmatter read/write
//! - [`render`]: `transcript.md` rendering
//! - [`store`]: create / read / list / update / delete items
//! - [`live`]: items written while a session runs (checkpoints, #153)
//! - [`meeting`]: what the meeting page said during a browser session (#126)
//! - [`subtitles`]: SRT/WebVTT writers (pure, #133)
//! - [`index`]: search index (rebuildable)

pub mod companion;
pub mod export;
pub mod external;
pub mod frontmatter;
pub mod index;
pub mod live;
pub mod meeting;
pub mod paths;
pub mod render;
pub mod store;
pub mod subtitles;
pub mod types;

pub use index::{rebuild_index, with_index, Index, SearchFilters};
pub use paths::resolve_archive_dir;
pub use store::{
    create_item, delete_item, edit_segment, edit_speakers, list_items, read_item, save_segments,
    update_meta, Item, ItemSummary, SegmentEdit, SpeakerEdit,
};
pub use types::{
    Channel, DocSpeaker, ItemMeta, ItemType, Participant, Segment, SegmentsFile, SessionState,
    Word, SESSION_KEY,
};

/// File name of the search index inside the app data dir.
pub const INDEX_FILE: &str = "archive-index.sqlite";
