//! The archive (plan §4.4): every non-dictation transcript becomes a folder
//! of plain files under `<Documents>/Sussurro`, with a derived SQLite FTS5
//! search index in the app data dir.
//!
//! - [`paths`]: location, folder naming, id validation and confinement
//! - [`types`]: frontmatter and `segments.json` data model
//! - [`audio`]: saved audio, one file per channel, written incrementally (#141)
//! - [`opus`]: the Ogg Opus writer, crash repair and reader of saved audio
//!   (#247, #248)
//! - [`compress`]: *Compress audio*, saved WAV files to Opus (#248)
//! - [`companion`]: recipe output next to the transcript (`document.md`, #120)
//! - [`export`]: `.md`/`.txt`/`.srt`/`.vtt` exports and `transcript.srt` (#133)
//! - [`external`]: what was sent to an external LLM host, per item (#122)
//! - [`frontmatter`]: YAML frontmatter read/write
//! - [`render`]: `transcript.md` rendering
//! - [`store`]: create / read / list / update / delete items
//! - [`live`]: items written while a session runs (checkpoints, #153)
//! - [`meeting`]: what the meeting page said during a browser session (#126)
//! - [`playback`]: saved audio served to the Audio tab's player (#142)
//! - [`subtitles`]: SRT/WebVTT writers (pure, #133)
//! - [`index`]: search index (rebuildable)
//! - [`facets`]: the Library's facet filters and counts (#135)

pub mod audio;
pub mod companion;
pub mod compress;
pub mod export;
pub mod external;
pub mod facets;
pub mod frontmatter;
pub mod index;
pub mod live;
pub mod meeting;
pub mod opus;
pub mod paths;
pub mod people;
pub mod playback;
pub mod render;
pub mod store;
pub mod subtitles;
pub mod types;

pub use index::{rebuild_index, with_index, Index, SearchFilters};
pub use paths::{prepare_archive_dir, resolve_archive_dir};
pub use store::{
    create_item, delete_item, edit_segment, edit_speakers, list_items, read_item, save_segments,
    update_meta, Item, ItemSummary, SegmentEdit, SpeakerEdit,
};
pub use types::{
    Channel, DocSpeaker, ItemMeta, ItemType, OverlapSpan, Participant, Segment, SegmentsFile,
    SessionState, Word, SESSION_KEY,
};

/// File name of the search index inside the app data dir.
pub const INDEX_FILE: &str = "archive-index.sqlite";
