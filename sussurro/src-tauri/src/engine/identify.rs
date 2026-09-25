//! "Identify voices" on a transcription already in the archive (P11, #134):
//! the same labelling as a run with the toggle on — each line's audio goes
//! through a [`Tracker`] clustering the `file` channel, then the end-of-run
//! fold ([`crate::speakers::doc::finalize_live`]) — but from the original
//! file, since the audio itself is never kept (P9). The original file is
//! known only for file transcriptions made from this version on
//! ([`super::source_files`]); a link's download is deleted after the run
//! and re-downloading it is out of scope. Without the original file, the
//! audio saved with the item (when Save audio was on; WAV or Opus, #248)
//! serves instead.

use super::source_files::{self, Entry, FileState};
use crate::archive::{self, Channel, ItemType, SegmentsFile};
use crate::sources::Source;
use crate::speakers::tracker::{EmbedderLoader, Labelled, SpeakerModels};
use crate::speakers::{SpeakerOptions, Tracker};
use anyhow::{bail, Result};
use serde::Serialize;
use std::path::Path;

/// One line to label: its id and time range on the file's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub segment_id: u32,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// The speaker the tracker gave one line.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceLabel {
    pub span: Span,
    pub labelled: Labelled,
}

/// What "Identify voices" did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Identified {
    /// Voices after the run.
    pub voices: usize,
    /// Lines that got a voice.
    pub lines: usize,
}

/// Whether "Identify voices" can run on an item, for the speaker panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceSource {
    pub available: bool,
    /// Why not, in words for the panel (empty when available).
    pub reason: String,
    /// The original file's name (empty when unknown), or the saved audio
    /// file's when `saved_audio`.
    pub file_name: String,
    /// The voices come from the audio saved with the item (#141, WAV or
    /// Opus, #248): the original file isn't available, the saved audio is.
    pub saved_audio: bool,
}

fn ms_to_samples(ms: u64) -> u64 {
    ms * super::segmenter::RATE / 1000
}

/// The lines "Identify voices" labels: transcribed lines of the `file`
/// channel (a line STT failed on has no voice either, as in a run).
pub fn spans(file: &SegmentsFile) -> Vec<Span> {
    let mut out: Vec<Span> = file
        .segments
        .iter()
        .filter(|s| s.channel == Channel::File && s.stt_error.is_none() && s.end_ms > s.start_ms)
        .map(|s| Span {
            segment_id: s.id,
            start_ms: s.start_ms,
            end_ms: s.end_ms,
        })
        .collect();
    out.sort_by_key(|s| (s.start_ms, s.segment_id));
    out
}

/// Stream `source` once and label every span, in time order, with
/// `tracker` — never the whole file in RAM, only the audio from the
/// earliest span not yet labelled. Fails when the file ends before the
/// last line (not the audio the transcript came from).
pub fn label_source(
    source: &mut dyn Source,
    spans: &[Span],
    tracker: &mut Tracker,
) -> Result<Vec<VoiceLabel>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut buf: Vec<f32> = Vec::new();
    // Sample index (file clock) of buf[0].
    let mut buf_start: u64 = 0;
    let mut next = 0usize;
    while next < spans.len() {
        let Some(frame) = source.next_frame()? else {
            break;
        };
        if frame.channel != Channel::File {
            continue;
        }
        let buf_end = buf_start + buf.len() as u64;
        if frame.start > buf_end {
            // A gap on the clock is silence (sources don't make any; kept
            // for safety so the ranges stay aligned).
            buf.resize(buf.len() + (frame.start - buf_end) as usize, 0.0);
        }
        let skip = (buf_start + buf.len() as u64).saturating_sub(frame.start) as usize;
        buf.extend_from_slice(frame.samples.get(skip..).unwrap_or_default());
        let buf_end = buf_start + buf.len() as u64;
        while let Some(span) = spans.get(next) {
            let (a, b) = (ms_to_samples(span.start_ms), ms_to_samples(span.end_ms));
            if b > buf_end {
                break;
            }
            let from = a.saturating_sub(buf_start) as usize;
            let to = (b - buf_start) as usize;
            let labelled = tracker.label(Channel::File, &buf[from.min(to)..to]);
            out.push(VoiceLabel {
                span: *span,
                labelled,
            });
            next += 1;
        }
        // Keep only what the next spans still need (they start in order).
        let keep_from = spans
            .get(next)
            .map(|s| ms_to_samples(s.start_ms))
            .unwrap_or(buf_end)
            .max(buf_start);
        let drop = ((keep_from - buf_start) as usize).min(buf.len());
        buf.drain(..drop);
        buf_start += drop as u64;
    }
    if next < spans.len() {
        bail!(
            "the file is shorter than the transcript — it is not the audio this transcription \
             was made from"
        );
    }
    Ok(out)
}

/// Put the labels on the document: speaker and embedding of each line
/// (only if the line is still the one that was labelled — same id and
/// time range), the new voices, then the end-of-run fold of tiny voices.
/// Refused when the document already has voice data ("Re-detect" is for
/// that) or when no line was long enough to tell a voice.
pub fn apply(file: &mut SegmentsFile, labels: &[VoiceLabel]) -> Result<Identified> {
    if file.segments.iter().any(|s| s.embedding.is_some()) {
        bail!("this transcription already has voice data — use Re-detect speakers");
    }
    let mut lines = 0;
    for l in labels {
        let (Some(speaker_id), Some(embedding)) = (&l.labelled.speaker_id, &l.labelled.embedding)
        else {
            continue;
        };
        let Some(seg) = file.segments.iter_mut().find(|s| {
            s.id == l.span.segment_id && s.start_ms == l.span.start_ms && s.end_ms == l.span.end_ms
        }) else {
            continue;
        };
        seg.speaker_id = Some(speaker_id.clone());
        seg.embedding = Some(embedding.clone());
        seg.overlap = l.labelled.overlap_at(l.span.start_ms);
        lines += 1;
        if let Some(sp) = &l.labelled.new_speaker {
            if !file.speakers.iter().any(|s| s.id == sp.id) {
                file.speakers.push(sp.clone());
            }
        }
    }
    if lines == 0 {
        bail!("no line is long enough to tell voices apart (at least 1 s of speech each)");
    }
    crate::speakers::doc::finalize_live(file);
    crate::speakers::overlap::assign_second_speakers(file);
    let voices = file
        .speakers
        .iter()
        .filter(|sp| {
            crate::speakers::doc::voice_number(&sp.id).is_some()
                && file
                    .segments
                    .iter()
                    .any(|s| s.speaker_id.as_deref() == Some(sp.id.as_str()))
        })
        .count();
    Ok(Identified { voices, lines })
}

fn file_name_of(source: &str) -> &str {
    source.strip_prefix("file:").unwrap_or_default()
}

/// The item's saved audio of the `file` channel, if any: `audio.<ext>` (a
/// file or link run's single channel) or `audio-file.<ext>`, WAV first
/// (lossless) when a Compress audio left both. Its clock is the file's, so
/// the lines' times fit it. Pure.
pub fn saved_voice_audio(item: &archive::Item) -> Option<String> {
    [
        "audio.wav",
        "audio-file.wav",
        "audio.opus",
        "audio-file.opus",
    ]
    .into_iter()
    .find(|name| item.audio.iter().any(|f| f.name == *name))
    .map(str::to_string)
}

/// Whether "Identify voices" can run on this item, and if not, why — in
/// words for the speaker panel. `original`: the recorded original file and
/// its state now, if any; without it the audio saved with the item (#141)
/// serves too. Pure.
pub fn availability(item: &archive::Item, original: Option<FileState>) -> VoiceSource {
    let source = item.meta.source.trim();
    let file_name = file_name_of(source).to_string();
    let no = |reason: String| VoiceSource {
        available: false,
        reason,
        file_name: file_name.clone(),
        saved_audio: false,
    };
    match item.meta.item_type {
        ItemType::Transcription => {}
        ItemType::Note => return no("Notes are your own voice: they have no speakers.".into()),
        ItemType::Meeting => return no("Meetings get their voices while they are recorded.".into()),
    }
    if item.recording {
        return no("Available when the recording ends.".into());
    }
    if item.edited_externally {
        return no("The transcript was edited outside Sussurro.".into());
    }
    if item.embedded_segments > 0 {
        return no("This transcription already has voice data: use Re-detect speakers.".into());
    }
    // The original file first (lossless); else the audio saved with the item.
    if original != Some(FileState::Available) || !source.starts_with("file:") {
        if let Some(saved) = saved_voice_audio(item) {
            return VoiceSource {
                available: true,
                reason: String::new(),
                file_name: saved,
                saved_audio: true,
            };
        }
    }
    if source.starts_with("url:") {
        return no(
            "Voices are found in the audio, and a link's download is deleted once it is \
             transcribed (downloading it again is not supported yet). Transcribe the link again \
             with Identify voices on."
                .into(),
        );
    }
    if !source.starts_with("file:") {
        return no("The original audio of this transcription is not available.".into());
    }
    match original {
        Some(FileState::Available) => VoiceSource {
            available: true,
            reason: String::new(),
            file_name,
            saved_audio: false,
        },
        Some(FileState::Missing) => no(format!(
            "The original file “{file_name}” is no longer where it was transcribed from. \
             Transcribe it again with Identify voices on."
        )),
        Some(FileState::Changed) => no(format!(
            "The file “{file_name}” changed since it was transcribed, so its voices would not \
             match the lines. Transcribe it again with Identify voices on."
        )),
        None => no(format!(
            "Sussurro doesn't know where “{file_name}” is: it was transcribed before voices \
             could be identified later, or on another computer. Transcribe it again with \
             Identify voices on."
        )),
    }
}

/// [`availability`] of item `id`, with its original file looked up in the
/// app data dir's `store`.
pub fn voice_source(archive: &Path, store: &Path, id: &str) -> Result<VoiceSource> {
    let item = archive::read_item(archive, id)?;
    let original = source_files::lookup(store, archive, id).map(|e| source_files::check(&e));
    Ok(availability(&item, original))
}

/// "Identify voices" on transcription `id`: re-reads its original file —
/// or, without it, the audio saved with the item (WAV or Opus) — labels
/// the lines as a run with the toggle on would, and saves. The
/// speaker model comes from `load` (downloaded on first use in the app); a
/// model that can't be loaded fails here, before anything changes.
/// Blocking — a long file takes a while.
pub fn identify_voices(
    archive: &Path,
    store: &Path,
    id: &str,
    load: EmbedderLoader,
) -> Result<(archive::Item, Identified)> {
    identify_voices_with_own_voice(archive, store, id, load.into(), None)
}

/// [`identify_voices`] with the run's models — the overlap model too, when
/// given (#244: overlapped lines get their spans and second speakers) —
/// then the user's enrolled voice (`you`, #243) labels its best match
/// "You" ([`crate::speakers::own_voice::label_you`]).
pub fn identify_voices_with_own_voice(
    archive: &Path,
    store: &Path,
    id: &str,
    models: SpeakerModels,
    you: Option<&[f32]>,
) -> Result<(archive::Item, Identified)> {
    let item = archive::read_item(archive, id)?;
    let entry: Option<Entry> = source_files::lookup(store, archive, id);
    let state = availability(&item, entry.as_ref().map(source_files::check));
    if !state.available {
        bail!("{}", state.reason);
    }
    let path = match entry {
        Some(entry) if !state.saved_audio => entry.path,
        // Confined to the item folder, a regular file (as the player's).
        _ => archive::playback::resolve(archive, id, &state.file_name)?,
    };
    let spans = spans(&item.segments);
    if spans.is_empty() {
        bail!("this transcription has no transcribed lines");
    }
    let embedder = (models.embedder)()?;
    let mut tracker = Tracker::new(
        SpeakerOptions::clustering(&[Channel::File]),
        Box::new(move || Ok(embedder)),
    )
    .with_overlap(models.overlap);
    let mut source = crate::sources::file::FileSource::open(&path)?;
    let labels = label_source(&mut source, &spans, &mut tracker)?;
    let mut identified = None;
    let item = archive::store::modify_segments(archive, id, |file| {
        identified = Some(apply(file, &labels)?);
        if let Some(you) = you {
            crate::speakers::own_voice::label_you(file, &item.meta.source, you);
        }
        Ok(())
    })?;
    let identified = identified.unwrap_or(Identified {
        voices: 0,
        lines: 0,
    });
    Ok((item, identified))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{DocSpeaker, ItemMeta, Segment};
    use crate::speakers::tracker::tests::fake_loader;

    /// 16 kHz mono stretches: `Some(v)` = voice `v` (level 0.2·(v+1), what
    /// the fake embedder keys on), `None` = silence.
    fn voiced(parts: &[(Option<usize>, f32)]) -> Vec<f32> {
        let mut out = Vec::new();
        for &(voice, secs) in parts {
            let n = (secs * 16_000.0) as usize;
            let amp = voice.map_or(0.0, |v| 0.2 * (v as f32 + 1.0));
            // 100 ms fades: a lossy copy (saved Opus, #248) then keeps the
            // peak the fake embedder keys on (a hard onset overshoots).
            let fade = |i: usize| (i.min(n - i) as f32 / 1_600.0).min(1.0);
            out.extend((0..n).map(|i| amp * fade(i) * ((i as f32) * 0.07).sin()));
        }
        out
    }

    fn seg(id: u32, start_ms: u64, end_ms: u64) -> Segment {
        Segment {
            id,
            channel: Channel::File,
            start_ms,
            end_ms,
            raw: format!("riga {id}"),
            text: format!("Riga {id}."),
            ..Default::default()
        }
    }

    /// Four 6 s lines, voices A B A B, with 3 s pauses (as a run would cut
    /// them), plus a line STT failed on.
    fn layout() -> (Vec<f32>, SegmentsFile) {
        let audio = voiced(&[
            (None, 1.0),
            (Some(0), 6.0),
            (None, 3.0),
            (Some(1), 6.0),
            (None, 3.0),
            (Some(0), 6.0),
            (None, 3.0),
            (Some(1), 6.0),
            (None, 1.0),
        ]);
        let mut failed = seg(4, 34_000, 34_500);
        failed.stt_error = Some("model missing".into());
        failed.text.clear();
        let file = SegmentsFile {
            segments: vec![
                seg(0, 1_000, 7_000),
                seg(1, 10_000, 16_000),
                seg(2, 19_000, 25_000),
                seg(3, 28_000, 34_000),
                failed,
            ],
            ..Default::default()
        };
        (audio, file)
    }

    fn write_item(archive: &Path, meta: &ItemMeta, file: &SegmentsFile) -> String {
        archive::create_item(archive, meta, file).unwrap()
    }

    fn transcription(source: &str) -> ItemMeta {
        ItemMeta {
            item_type: ItemType::Transcription,
            title: "Intervista".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            source: source.into(),
            ..Default::default()
        }
    }

    fn who(file: &SegmentsFile) -> Vec<Option<&str>> {
        file.segments
            .iter()
            .map(|s| s.speaker_id.as_deref())
            .collect()
    }

    #[test]
    fn spans_skip_failed_and_other_channel_lines_in_time_order() {
        let (_, mut file) = layout();
        file.segments.swap(0, 2);
        let mut mic = seg(9, 2_000, 3_000);
        mic.channel = Channel::Mic;
        file.segments.push(mic);
        let s = spans(&file);
        assert_eq!(
            s.iter().map(|s| s.segment_id).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn identify_labels_a_stored_transcription_from_its_file() {
        let tmp = tempfile::tempdir().unwrap();
        let archive_dir = tmp.path().join("Sussurro");
        let store = tmp.path().join("appdata").join(source_files::FILE);
        let wav = tmp.path().join("Intervista.wav");
        let (audio, file) = layout();
        crate::audio::decode::write_wav_i16(&wav, 16_000, 1, &audio);
        let id = write_item(&archive_dir, &transcription("file:Intervista.wav"), &file);

        // Not recorded yet (an item from before #134): explained, refused.
        let vs = voice_source(&archive_dir, &store, &id).unwrap();
        assert!(
            !vs.available && vs.reason.contains("doesn't know where"),
            "{vs:?}"
        );
        assert_eq!(vs.file_name, "Intervista.wav");
        let (load, calls) = fake_loader(5);
        assert!(identify_voices(&archive_dir, &store, &id, load).is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);

        source_files::record(&store, &archive_dir, &id, &wav).unwrap();
        assert!(voice_source(&archive_dir, &store, &id).unwrap().available);
        let (load, calls) = fake_loader(5);
        let (item, done) = identify_voices(&archive_dir, &store, &id, load).unwrap();
        assert!(calls.load(std::sync::atomic::Ordering::Relaxed) > 0);
        assert_eq!(
            done,
            Identified {
                voices: 2,
                lines: 4
            }
        );
        assert_eq!(
            who(&item.segments),
            [
                Some("voice:1"),
                Some("voice:2"),
                Some("voice:1"),
                Some("voice:2"),
                None
            ]
        );
        let ids: Vec<&str> = item
            .segments
            .speakers
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(ids, ["voice:1", "voice:2"]);
        assert_eq!(item.embedded_segments, 4);
        assert!(item.body.contains("Voice 2:**"), "{}", item.body);

        // #133: the subtitles of a transcription carry the voice names.
        let srt =
            archive::export::export_item(&archive_dir, &id, archive::export::ExportFormat::Srt)
                .unwrap();
        assert!(srt.contains("Voice 1: Riga 0."), "{srt}");
        assert!(srt.contains("Voice 2: Riga 1."), "{srt}");

        // Now it has voice data: Re-detect is the way, not Identify again.
        let vs = voice_source(&archive_dir, &store, &id).unwrap();
        assert!(!vs.available && vs.reason.contains("Re-detect"), "{vs:?}");
        let (load, _) = fake_loader(5);
        assert!(identify_voices(&archive_dir, &store, &id, load).is_err());
        let redetected =
            archive::edit_speakers(&archive_dir, &id, archive::SpeakerEdit::Redetect).unwrap();
        assert_eq!(redetected.embedded_segments, 4);
    }

    #[test]
    fn a_shorter_or_replaced_file_is_refused_and_nothing_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let archive_dir = tmp.path().join("Sussurro");
        let store = tmp.path().join(source_files::FILE);
        let wav = tmp.path().join("a.wav");
        let (audio, file) = layout();
        let id = write_item(&archive_dir, &transcription("file:a.wav"), &file);

        // Replaced by a shorter recording after it was transcribed.
        crate::audio::decode::write_wav_i16(&wav, 16_000, 1, &audio);
        source_files::record(&store, &archive_dir, &id, &wav).unwrap();
        crate::audio::decode::write_wav_i16(&wav, 16_000, 1, &audio[..16_000 * 20]);
        let vs = voice_source(&archive_dir, &store, &id).unwrap();
        assert!(!vs.available && vs.reason.contains("changed"), "{vs:?}");

        // Same size check passes, but the audio ends before the last line.
        source_files::record(&store, &archive_dir, &id, &wav).unwrap();
        let (load, _) = fake_loader(5);
        let err = identify_voices(&archive_dir, &store, &id, load).unwrap_err();
        assert!(
            err.to_string().contains("shorter than the transcript"),
            "{err}"
        );
        let item = archive::read_item(&archive_dir, &id).unwrap();
        assert_eq!(item.embedded_segments, 0);
        assert!(item.segments.speakers.is_empty());

        std::fs::remove_file(&wav).unwrap();
        let vs = voice_source(&archive_dir, &store, &id).unwrap();
        assert!(!vs.available && vs.reason.contains("no longer"), "{vs:?}");
    }

    /// #248: without the original file, the audio saved with the item —
    /// here Opus, decoded by libopus — gives the voices back, for a link
    /// transcription too.
    #[test]
    fn identify_reads_the_saved_opus_audio_when_the_original_is_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let archive_dir = tmp.path().join("Sussurro");
        let store = tmp.path().join(source_files::FILE);
        let (audio, file) = layout();
        for source in ["file:gone.wav", "url:https://example.org/a.mp3"] {
            let id = write_item(&archive_dir, &transcription(source), &file);
            let vs = voice_source(&archive_dir, &store, &id).unwrap();
            assert!(!vs.available && !vs.saved_audio, "{vs:?}");

            let saved = archive_dir.join(&id).join("audio.opus");
            let mut w = archive::opus::OpusWriter::create_capped(&saved, u64::MAX).unwrap();
            w.write(&audio).unwrap();
            w.finish().unwrap();
            // The decoded lines keep their level well inside the fake
            // embedder's bands (0.2 → voice 1, 0.4 → voice 2; edges ±0.1).
            let (pcm, _) = archive::opus::decode_file(&saved).unwrap();
            for (seg, want) in file.segments.iter().take(4).zip([0.2f32, 0.4, 0.2, 0.4]) {
                let a = ms_to_samples(seg.start_ms) as usize;
                let b = ms_to_samples(seg.end_ms) as usize;
                let peak = pcm[a..b].iter().fold(0f32, |m, x| m.max(x.abs()));
                assert!((peak - want).abs() < 0.06, "line {}: peak {peak}", seg.id);
            }
            let vs = voice_source(&archive_dir, &store, &id).unwrap();
            assert!(vs.available && vs.saved_audio, "{vs:?}");
            assert_eq!(vs.file_name, "audio.opus");

            let (load, _) = fake_loader(5);
            let (item, done) = identify_voices(&archive_dir, &store, &id, load).unwrap();
            assert_eq!(
                done,
                Identified {
                    voices: 2,
                    lines: 4
                },
                "{source}"
            );
            assert_eq!(
                who(&item.segments),
                [
                    Some("voice:1"),
                    Some("voice:2"),
                    Some("voice:1"),
                    Some("voice:2"),
                    None
                ]
            );
        }
    }

    #[test]
    fn a_model_that_fails_to_load_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let archive_dir = tmp.path().join("Sussurro");
        let store = tmp.path().join(source_files::FILE);
        let wav = tmp.path().join("a.wav");
        let (audio, file) = layout();
        crate::audio::decode::write_wav_i16(&wav, 16_000, 1, &audio);
        let id = write_item(&archive_dir, &transcription("file:a.wav"), &file);
        source_files::record(&store, &archive_dir, &id, &wav).unwrap();
        let load: EmbedderLoader = Box::new(|| bail!("speaker model not downloaded"));
        let err = identify_voices(&archive_dir, &store, &id, load).unwrap_err();
        assert!(err.to_string().contains("not downloaded"), "{err}");
        assert_eq!(
            archive::read_item(&archive_dir, &id)
                .unwrap()
                .embedded_segments,
            0
        );
    }

    fn item_with(meta: ItemMeta) -> archive::Item {
        archive::Item {
            id: "2026/09/x".into(),
            meta,
            segments: SegmentsFile::default(),
            body: String::new(),
            edited_externally: false,
            recording: false,
            interrupted: false,
            external_hosts: Vec::new(),
            embedded_segments: 0,
            audio: Vec::new(),
            folder_bytes: 0,
        }
    }

    #[test]
    fn availability_explains_every_case() {
        let ok = |i: &archive::Item, f: Option<FileState>| availability(i, f).available;
        let t = item_with(transcription("file:a.wav"));
        assert!(ok(&t, Some(FileState::Available)));
        assert!(!ok(&t, None));
        assert!(!ok(&t, Some(FileState::Missing)));
        assert!(!ok(&t, Some(FileState::Changed)));

        let mut note = t.clone();
        note.meta.item_type = ItemType::Note;
        assert!(!ok(&note, Some(FileState::Available)));
        let mut meeting = t.clone();
        meeting.meta.item_type = ItemType::Meeting;
        assert!(!ok(&meeting, Some(FileState::Available)));

        let link = item_with(transcription("url:https://example.org/a.mp3"));
        let v = availability(&link, None);
        assert!(
            !v.available && v.reason.contains("not supported yet"),
            "{v:?}"
        );
        assert_eq!(v.file_name, "");

        let mut rec = t.clone();
        rec.recording = true;
        assert!(!ok(&rec, Some(FileState::Available)));
        let mut ext = t.clone();
        ext.edited_externally = true;
        assert!(!ok(&ext, Some(FileState::Available)));
        let mut done = t.clone();
        done.embedded_segments = 3;
        assert!(!ok(&done, Some(FileState::Available)));

        // Saved audio (#141, #248) stands in for a missing original file.
        let with = |i: &archive::Item, names: &[&str]| {
            let mut i = i.clone();
            i.audio = names
                .iter()
                .map(|n| archive::audio::AudioFile {
                    name: n.to_string(),
                    bytes: 1,
                })
                .collect();
            i
        };
        let saved = with(&t, &["audio.opus"]);
        for f in [None, Some(FileState::Missing), Some(FileState::Changed)] {
            let v = availability(&saved, f.clone());
            assert!(v.available && v.saved_audio, "{f:?}");
            assert_eq!(v.file_name, "audio.opus");
        }
        // The original, when there, comes first (lossless).
        let v = availability(&saved, Some(FileState::Available));
        assert!(v.available && !v.saved_audio);
        assert_eq!(v.file_name, "a.wav");
        assert!(availability(&with(&link, &["audio.wav"]), None).saved_audio);
        // WAV before Opus (a Compress audio cut short leaves both).
        let both = with(&t, &["audio.opus", "audio.wav"]);
        assert_eq!(availability(&both, None).file_name, "audio.wav");
        // Other channels' audio is not the file's.
        assert!(!ok(&with(&t, &["audio-mic.opus"]), None));
        // Every other refusal still wins.
        let mut rec = with(&t, &["audio.wav"]);
        rec.recording = true;
        assert!(!ok(&rec, None));
        let mut done = with(&t, &["audio.wav"]);
        done.embedded_segments = 3;
        assert!(!ok(&done, None));
    }

    #[test]
    fn apply_keeps_lines_edited_meanwhile_and_existing_names() {
        let (_, mut file) = layout();
        file.speakers.push(DocSpeaker {
            id: "voice:1".into(),
            label: "Anna".into(),
            color: "#123456".into(),
            person_id: None,
            ..Default::default()
        });
        let emb = vec![1.0f32; 4];
        let label = |id: u32, start: u64, end: u64, voice: &str, new: bool| VoiceLabel {
            span: Span {
                segment_id: id,
                start_ms: start,
                end_ms: end,
            },
            labelled: Labelled {
                speaker_id: Some(voice.into()),
                embedding: Some(emb.clone()),
                new_speaker: new.then(|| crate::speakers::doc::voice_speaker(1)),
                overlap: Vec::new(),
            },
        };
        let labels = vec![
            label(0, 1_000, 7_000, "voice:1", true),
            // Line 1 was re-timed meanwhile: left alone.
            label(1, 10_000, 15_000, "voice:1", false),
            VoiceLabel {
                span: Span {
                    segment_id: 2,
                    start_ms: 19_000,
                    end_ms: 25_000,
                },
                labelled: Labelled::default(),
            },
        ];
        let done = apply(&mut file, &labels).unwrap();
        assert_eq!(done.lines, 1);
        assert_eq!(who(&file), [Some("voice:1"), None, None, None, None]);
        assert_eq!(
            file.speakers.len(),
            1,
            "the listed voice is not added twice"
        );
        assert!(
            apply(&mut file, &labels).is_err(),
            "voice data now: Re-detect"
        );

        let (_, mut empty) = layout();
        assert!(apply(&mut empty, &[]).is_err());
    }

    /// Runs the real model on a two-voice WAV (16 kHz mono, 16-bit) cut
    /// into 5 s lines, as "Identify voices" would label a transcription of
    /// it, and checks that at least two voices come out:
    ///
    /// ```text
    /// SUSSURRO_SPEAKER_MODEL=<voxceleb_resnet34_LM.onnx> \
    /// SUSSURRO_SPEAKER_WAV=<two speakers, ~1 min> \
    ///   cargo test engine::identify::tests::real_model -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs the model file and a two-voice WAV"]
    fn real_model_identifies_two_voices_in_a_file() {
        use crate::speakers::model::{SpeakerEmbedder, WeSpeaker};
        let model = std::path::PathBuf::from(std::env::var("SUSSURRO_SPEAKER_MODEL").unwrap());
        let wav = std::path::PathBuf::from(std::env::var("SUSSURRO_SPEAKER_WAV").unwrap());
        let probe = crate::sources::file::FileSource::open(&wav).unwrap();
        let total_ms = probe.total_samples().unwrap() / 16;
        assert!(total_ms >= 20_000, "need at least 20 s of audio");
        let file = SegmentsFile {
            segments: (0..total_ms / 5_000)
                .map(|i| seg(i as u32, i * 5_000, i * 5_000 + 5_000))
                .collect(),
            ..Default::default()
        };
        let tmp = tempfile::tempdir().unwrap();
        let archive_dir = tmp.path().join("Sussurro");
        let store = tmp.path().join(source_files::FILE);
        let id = write_item(&archive_dir, &transcription("file:two.wav"), &file);
        source_files::record(&store, &archive_dir, &id, &wav).unwrap();
        let load: EmbedderLoader =
            Box::new(move || Ok(Box::new(WeSpeaker::load(&model)?) as Box<dyn SpeakerEmbedder>));
        let (item, done) = identify_voices(&archive_dir, &store, &id, load).unwrap();
        eprintln!("{done:?}: {:?}", who(&item.segments));
        assert!(done.voices >= 2, "one voice only: {done:?}");
    }
}
