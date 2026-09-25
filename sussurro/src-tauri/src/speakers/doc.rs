//! Speakers of one document (pure operations on a [`SegmentsFile`]):
//! labels and colours, moving a line to another speaker, renaming a
//! speaker for this document, "Re-detect speakers", and the tidy-up at the
//! end of a live session. The archive store runs these under its freshness
//! rules (`archive::store::edit_speakers`); nothing here touches the disk.

use super::cluster::{agglomerative, fold_small, renumber_by_first_appearance};
use super::{MIN_VOICE_SPEECH_MS, REDETECT_THRESHOLD};
use crate::archive::people::{name_key, Person};
use crate::archive::types::{normalize_participants, Participant};
use crate::archive::{DocSpeaker, ItemMeta, Segment, SegmentsFile};
use anyhow::{bail, Result};

/// Speaker id of the user on the mic channel of a two-channel session.
pub const YOU_ID: &str = "you";
/// Prefix of the acoustic voices' ids (`voice:1`, `voice:2`, …).
pub const VOICE_PREFIX: &str = "voice:";
/// Target of a line move that opens a new voice.
pub const NEW_VOICE: &str = "voice:new";
/// Longest speaker label, in characters.
pub const MAX_LABEL_CHARS: usize = 60;

/// Colour of "You" (the mock's `--sp-you`).
pub const YOU_COLOR: &str = "#1a1a1a";
/// Voice colours, cycled by voice number. Dark enough for white text on
/// both themes; the first two match the approved mock (`--sp-3`, `--sp-4`).
pub const VOICE_COLORS: [&str; 8] = [
    "#0f766e", "#7e22ce", "#1f6feb", "#c2410c", "#be185d", "#4d7c0f", "#0369a1", "#9a3412",
];

pub fn voice_id(n: u32) -> String {
    format!("{VOICE_PREFIX}{n}")
}

/// The number of a `voice:<n>` id (`n ≥ 1`).
pub fn voice_number(id: &str) -> Option<u32> {
    id.strip_prefix(VOICE_PREFIX)?
        .parse()
        .ok()
        .filter(|n| *n > 0)
}

pub fn voice_label(n: u32) -> String {
    format!("Voice {n}")
}

pub fn voice_color(n: u32) -> String {
    VOICE_COLORS[(n.max(1) as usize - 1) % VOICE_COLORS.len()].to_string()
}

/// A fresh "Voice N".
pub fn voice_speaker(n: u32) -> DocSpeaker {
    DocSpeaker {
        id: voice_id(n),
        label: voice_label(n),
        color: voice_color(n),
        person_id: None,
        label_before_link: None,
        own_voice: None,
    }
}

/// The user, on the mic channel of a two-channel session.
pub fn you_speaker() -> DocSpeaker {
    DocSpeaker {
        id: YOU_ID.to_string(),
        label: "You".to_string(),
        color: YOU_COLOR.to_string(),
        person_id: None,
        label_before_link: None,
        own_voice: None,
    }
}

/// Prefix of the names from the meeting page (`meet:Anna Rossi`, #131).
pub const MEET_PREFIX: &str = "meet:";

pub fn meet_id(name: &str) -> String {
    format!("{MEET_PREFIX}{name}")
}

pub fn is_meet(id: &str) -> bool {
    id.starts_with(MEET_PREFIX)
}

/// A name from the meeting page. `k` is its order among the document's
/// names: they take the voice colours from the other end of the list, so
/// a small meeting's names and voices don't share a colour.
pub fn meet_speaker(name: &str, k: usize) -> DocSpeaker {
    DocSpeaker {
        id: meet_id(name),
        label: name.to_string(),
        color: VOICE_COLORS[VOICE_COLORS.len() - 1 - k % VOICE_COLORS.len()].to_string(),
        person_id: None,
        label_before_link: None,
        own_voice: None,
    }
}

/// Make the speaker list match the lines after the page's names were
/// applied (#131): an entry for every name and voice a line uses (new
/// ones get the defaults; existing entries keep their label and link),
/// none for names or voices no line uses. Other speakers ("You") stay
/// first, then the names in order of first appearance, then the voices by
/// number.
pub fn sync_named_speakers(file: &mut SegmentsFile) {
    let mut names: Vec<String> = Vec::new();
    for id in file.segments.iter().filter_map(|s| s.speaker_id.as_deref()) {
        if is_meet(id) && !names.iter().any(|n| n == id) {
            names.push(id.to_string());
        }
    }
    let old = std::mem::take(&mut file.speakers);
    let (meets, rest): (Vec<DocSpeaker>, Vec<DocSpeaker>) =
        old.into_iter().partition(|s| is_meet(&s.id));
    file.speakers = rest;
    let at = file
        .speakers
        .iter()
        .position(|s| voice_number(&s.id).is_some())
        .unwrap_or(file.speakers.len());
    let entries: Vec<DocSpeaker> = names
        .iter()
        .enumerate()
        .map(|(k, id)| {
            meets
                .iter()
                .find(|s| &s.id == id)
                .cloned()
                .unwrap_or_else(|| meet_speaker(id.strip_prefix(MEET_PREFIX).unwrap_or(id), k))
        })
        .collect();
    file.speakers.splice(at..at, entries);
    tidy_voices(file);
}

fn is_voice(speaker: Option<&str>) -> bool {
    speaker.is_some_and(|s| voice_number(s).is_some())
}

fn has_speaker(file: &SegmentsFile, id: &str) -> bool {
    file.speakers.iter().any(|s| s.id == id)
}

/// Highest voice number the document knows (speakers or lines), 0 if none.
fn max_voice(file: &SegmentsFile) -> u32 {
    let from_speakers = file.speakers.iter().filter_map(|s| voice_number(&s.id));
    let from_lines = file
        .segments
        .iter()
        .filter_map(|s| s.speaker_id.as_deref().and_then(voice_number));
    from_speakers.chain(from_lines).max().unwrap_or(0)
}

fn duration(s: &Segment) -> u64 {
    s.end_ms.saturating_sub(s.start_ms)
}

/// Move one line to another speaker of this document. `speaker_id` is an
/// existing speaker's id, or [`NEW_VOICE`] to open the next "Voice N".
/// Returns the id the line now has.
pub fn move_segment(file: &mut SegmentsFile, segment_id: u32, speaker_id: &str) -> Result<String> {
    let pos = file
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .ok_or_else(|| anyhow::anyhow!("no line {segment_id} in this document"))?;
    let target = if speaker_id == NEW_VOICE {
        let sp = voice_speaker(max_voice(file) + 1);
        let id = sp.id.clone();
        file.speakers.push(sp);
        id
    } else if has_speaker(file, speaker_id) {
        speaker_id.to_string()
    } else {
        bail!("no speaker '{speaker_id}' in this document");
    };
    file.segments[pos].speaker_id = Some(target.clone());
    Ok(target)
}

/// Rename a speaker for this document only. An empty label gives a voice
/// back its "Voice N" name; other speakers need a name.
pub fn rename_speaker(file: &mut SegmentsFile, speaker_id: &str, label: &str) -> Result<()> {
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.chars().count() > MAX_LABEL_CHARS {
        bail!("a speaker name can be at most {MAX_LABEL_CHARS} characters");
    }
    let sp = file
        .speakers
        .iter_mut()
        .find(|s| s.id == speaker_id)
        .ok_or_else(|| anyhow::anyhow!("no speaker '{speaker_id}' in this document"))?;
    sp.label = match (label.is_empty(), voice_number(speaker_id)) {
        (false, _) => label,
        (true, Some(n)) => voice_label(n),
        (true, None) => bail!("a speaker needs a name"),
    };
    // A name chosen by hand is the one an unlink keeps.
    sp.label_before_link = None;
    // Renaming a voice labelled "You" automatically is the user's call:
    // it is never labelled "You" automatically again (#243).
    if sp.own_voice == Some(true) {
        sp.own_voice = Some(false);
        if let Some(n) = voice_number(speaker_id) {
            sp.color = voice_color(n);
        }
    }
    Ok(())
}

/// The label a speaker gets without a user's choice: "Voice N", "You",
/// or the name the meeting page showed (`meet:<name>`).
pub fn default_label(id: &str) -> Option<String> {
    if let Some(n) = voice_number(id) {
        return Some(voice_label(n));
    }
    if id == YOU_ID {
        return Some("You".to_string());
    }
    id.strip_prefix("meet:").map(str::to_string)
}

/// Whether the user named this speaker (its label is not the default).
pub fn renamed(sp: &DocSpeaker) -> bool {
    // "You" from the own-voice match (#243) is not the user's choice.
    if sp.own_voice == Some(true) {
        return false;
    }
    default_label(&sp.id).is_none_or(|d| name_key(&d) != name_key(&sp.label))
}

/// Link a speaker to a person of the People registry (#132): it gets the
/// person's id and — unless the user named the speaker — the person's name
/// as its label (the old label is kept for [`unlink_speaker`]). The
/// person becomes a participant of the item with name and email: a
/// participant already naming them (or carrying the speaker's old generic
/// label, like "Voice 2") is completed, otherwise one is added. An
/// existing email is never replaced. Notes have no participants (P10).
pub fn link_speaker(
    file: &mut SegmentsFile,
    meta: &mut ItemMeta,
    speaker_id: &str,
    person: &Person,
) -> Result<()> {
    let sp = file
        .speakers
        .iter_mut()
        .find(|s| s.id == speaker_id)
        .ok_or_else(|| anyhow::anyhow!("no speaker '{speaker_id}' in this document"))?;
    // A voice labelled "You" automatically (#243) that the user links to
    // someone is not the user: it goes back to "Voice N" first and is never
    // labelled "You" automatically again.
    if sp.own_voice == Some(true) {
        sp.own_voice = Some(false);
        if let Some(n) = voice_number(&sp.id) {
            sp.label = voice_label(n);
            sp.color = voice_color(n);
        }
    }
    let old_label = sp.label.clone();
    if sp.person_id.as_deref() != Some(person.id.as_str()) {
        if sp.person_id.is_some() {
            // Re-linking to someone else starts from the pre-link label.
            if let Some(before) = sp.label_before_link.take() {
                sp.label = before;
            }
        }
        sp.person_id = Some(person.id.clone());
        if !renamed(sp) {
            sp.label_before_link = Some(std::mem::replace(&mut sp.label, person.name.clone()));
        }
    }
    if meta.item_type.has_participants() {
        link_participant(&mut meta.participants, person, &old_label);
    }
    Ok(())
}

/// Add `person` to the participants, or complete the entry that is them.
fn link_participant(list: &mut Vec<Participant>, person: &Person, old_label: &str) {
    let email_of = |p: &Participant| p.email.as_deref().map(|e| e.trim().to_lowercase());
    let person_email = person.email.as_deref().map(|e| e.trim().to_lowercase());
    let generic_old = !old_label.trim().is_empty() && name_key(old_label) != name_key(&person.name);
    let at = list
        .iter()
        .position(|p| {
            person.matches(&p.name) || (person_email.is_some() && email_of(p) == person_email)
        })
        .or_else(|| {
            generic_old
                .then(|| {
                    list.iter()
                        .position(|p| name_key(&p.name) == name_key(old_label))
                })
                .flatten()
        });
    match at {
        Some(i) => {
            let p = &mut list[i];
            // "Voice 2" becomes the person; a real name stays as typed.
            if !person.matches(&p.name) && name_key(&p.name) == name_key(old_label) {
                p.name = person.name.clone();
            }
            if p.email.as_deref().is_none_or(|e| e.trim().is_empty()) {
                p.email = person.email.clone();
            }
        }
        None => list.push(Participant {
            name: person.name.clone(),
            email: person.email.clone(),
        }),
    }
    *list = normalize_participants(list);
}

/// Undo [`link_speaker`]: the speaker forgets the person and gets back the
/// label it had before the link (a name the user gave it stays).
/// Participants are left as they are: the person was still in the room.
pub fn unlink_speaker(file: &mut SegmentsFile, speaker_id: &str) -> Result<()> {
    let sp = file
        .speakers
        .iter_mut()
        .find(|s| s.id == speaker_id)
        .ok_or_else(|| anyhow::anyhow!("no speaker '{speaker_id}' in this document"))?;
    sp.person_id = None;
    if let Some(before) = sp.label_before_link.take() {
        sp.label = before;
    }
    Ok(())
}

/// Speakers worth offering a link to (#130): not linked yet, and whose
/// label matches exactly one person — a name from the meeting page (#131)
/// or a rename that is someone in People. Returns `(speaker id, person)`.
pub fn link_suggestions<'a>(
    file: &SegmentsFile,
    people: &'a [Person],
) -> Vec<(String, &'a Person)> {
    file.speakers
        .iter()
        .filter(|s| s.person_id.is_none())
        .filter_map(|s| {
            crate::archive::people::match_person(people, &s.label).map(|p| (s.id.clone(), p))
        })
        .collect()
}

/// What "Re-detect speakers" did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Redetected {
    /// Voices after the run.
    pub voices: usize,
    /// Lines whose speaker changed.
    pub changed: usize,
}

/// Lines "Re-detect" may relabel: an embedding, and no speaker or a voice
/// (never "You" or a name from the meeting page).
fn redetect_candidates(file: &SegmentsFile) -> Vec<usize> {
    let dim = file
        .segments
        .iter()
        .find_map(|s| s.embedding.as_ref().map(Vec::len))
        .unwrap_or(0);
    file.segments
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.embedding
                .as_ref()
                .is_some_and(|e| e.len() == dim && dim > 0)
                && (s.speaker_id.is_none() || is_voice(s.speaker_id.as_deref()))
        })
        .map(|(i, _)| i)
        .collect()
}

/// "Re-detect speakers": offline agglomerative clustering of the whole
/// document on the stored per-line embeddings ([`REDETECT_THRESHOLD`]),
/// then voices under [`MIN_VOICE_SPEECH_MS`] of speech fold into the
/// nearest voice.
///
/// Labels stay stable: each new cluster takes the id of the voice it
/// shares the most speech with (so a renamed voice keeps its name), and
/// clusters matching no existing voice get new numbers after the highest
/// one ever used. Running it twice gives the same result (the clustering
/// depends only on the embeddings). Voices no line uses any more are
/// dropped; "You" and names from the meeting page are never touched.
///
/// Overlapped lines (#244) take part like any other (leaving them out
/// gained nothing in spike #237 and lost a quiet speaker); afterwards each
/// overlap span gets its second speaker again from the new voices
/// ([`super::overlap::assign_second_speakers`]).
pub fn redetect(file: &mut SegmentsFile) -> Result<Redetected> {
    let idx = redetect_candidates(file);
    if idx.is_empty() {
        bail!(
            "this document has no voice data — speakers can only be detected on recordings \
             made with speaker detection on"
        );
    }
    let embs: Vec<&[f32]> = idx
        .iter()
        .map(|&i| file.segments[i].embedding.as_deref().unwrap_or_default())
        .collect();
    let durs: Vec<u64> = idx.iter().map(|&i| duration(&file.segments[i])).collect();
    let labels = agglomerative(&embs, REDETECT_THRESHOLD);
    let labels =
        renumber_by_first_appearance(&fold_small(&embs, &durs, &labels, MIN_VOICE_SPEECH_MS));
    let nc = labels.iter().max().map_or(0, |m| m + 1);

    // Speech each new cluster shares with each current voice.
    let mut overlap: Vec<(u64, usize, u32)> = Vec::new();
    for (k, &i) in idx.iter().enumerate() {
        if let Some(n) = file.segments[i]
            .speaker_id
            .as_deref()
            .and_then(voice_number)
        {
            match overlap
                .iter_mut()
                .find(|(_, c, v)| *c == labels[k] && *v == n)
            {
                Some(o) => o.0 += durs[k],
                None => overlap.push((durs[k], labels[k], n)),
            }
        }
    }
    // Most shared speech first; ties to the earlier cluster, lower voice.
    overlap.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut number: Vec<Option<u32>> = vec![None; nc];
    let mut taken: Vec<u32> = Vec::new();
    for (_, c, n) in overlap {
        if number[c].is_none() && !taken.contains(&n) {
            number[c] = Some(n);
            taken.push(n);
        }
    }
    let mut next = max_voice(file);
    let number: Vec<u32> = number
        .into_iter()
        .map(|n| {
            n.unwrap_or_else(|| {
                next += 1;
                next
            })
        })
        .collect();

    let mut changed = 0;
    for (k, &i) in idx.iter().enumerate() {
        let id = voice_id(number[labels[k]]);
        let seg = &mut file.segments[i];
        if seg.speaker_id.as_deref() != Some(id.as_str()) {
            seg.speaker_id = Some(id);
            changed += 1;
        }
    }
    tidy_voices(file);
    super::overlap::assign_second_speakers(file);
    Ok(Redetected {
        voices: nc,
        changed,
    })
}

/// Keep one speaker entry per voice a line uses (a fresh "Voice N" for
/// voices without one), drop voices no line uses, and list the voices
/// after the other speakers, by number.
fn tidy_voices(file: &mut SegmentsFile) {
    let mut used: Vec<u32> = file
        .segments
        .iter()
        .filter_map(|s| s.speaker_id.as_deref().and_then(voice_number))
        .collect();
    used.sort_unstable();
    used.dedup();
    let old = std::mem::take(&mut file.speakers);
    let (voices, others): (Vec<DocSpeaker>, Vec<DocSpeaker>) =
        old.into_iter().partition(|s| voice_number(&s.id).is_some());
    file.speakers = others;
    for n in used {
        let id = voice_id(n);
        let sp = voices
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .unwrap_or_else(|| voice_speaker(n));
        file.speakers.push(sp);
    }
}

/// End of a live session: voices with less than [`MIN_VOICE_SPEECH_MS`] of
/// speech fold into the nearest voice (the online clustering over-splits,
/// #107), then the remaining voices are numbered 1, 2, … in order of first
/// appearance, with their default names and colours. Only lines carrying
/// an embedding take part; a no-op when nothing folds and the numbers are
/// already contiguous.
pub fn finalize_live(file: &mut SegmentsFile) {
    let idx: Vec<usize> = file
        .segments
        .iter()
        .enumerate()
        .filter(|(_, s)| s.embedding.is_some() && is_voice(s.speaker_id.as_deref()))
        .map(|(i, _)| i)
        .collect();
    if idx.is_empty() {
        return;
    }
    let embs: Vec<&[f32]> = idx
        .iter()
        .map(|&i| file.segments[i].embedding.as_deref().unwrap_or_default())
        .collect();
    let durs: Vec<u64> = idx.iter().map(|&i| duration(&file.segments[i])).collect();
    let numbers: Vec<u32> = idx
        .iter()
        .map(|&i| {
            file.segments[i]
                .speaker_id
                .as_deref()
                .and_then(voice_number)
                .unwrap_or(1)
        })
        .collect();
    let labels: Vec<usize> = numbers.iter().map(|n| *n as usize - 1).collect();
    let folded = fold_small(&embs, &durs, &labels, MIN_VOICE_SPEECH_MS);
    let compact = renumber_by_first_appearance(&folded);
    let unchanged = compact
        .iter()
        .zip(&numbers)
        .all(|(c, n)| *c as u32 + 1 == *n);
    if unchanged {
        return;
    }
    for (k, &i) in idx.iter().enumerate() {
        file.segments[i].speaker_id = Some(voice_id(compact[k] as u32 + 1));
    }
    // Live sessions can't be renamed, so the voices take default entries.
    file.speakers.retain(|s| voice_number(&s.id).is_none());
    tidy_voices(file);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speakers::cluster::tests::{sample, voices};

    fn seg(id: u32, start: u64, len: u64, speaker: Option<&str>, emb: Option<Vec<f32>>) -> Segment {
        Segment {
            id,
            start_ms: start,
            end_ms: start + len,
            speaker_id: speaker.map(str::to_string),
            text: format!("line {id}"),
            embedding: emb,
            ..Default::default()
        }
    }

    /// A document of `truth[i]` speakers, 4 s per line, with embeddings,
    /// labelled with `labels[i]` (a voice number, or `None`).
    fn doc(truth: &[usize], labels: &[Option<u32>], seed: u64) -> SegmentsFile {
        let (mut rng, v) = voices(4, seed);
        let segments = truth
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(i, (&t, l))| {
                let id = l.map(voice_id);
                seg(
                    i as u32,
                    i as u64 * 5000,
                    4000,
                    id.as_deref(),
                    Some(sample(&mut rng, &v[t], 0.9)),
                )
            })
            .collect();
        let mut speakers: Vec<DocSpeaker> =
            labels.iter().flatten().map(|n| voice_speaker(*n)).collect();
        speakers.sort_by(|a, b| a.id.cmp(&b.id));
        speakers.dedup();
        SegmentsFile {
            speakers,
            segments,
            ..Default::default()
        }
    }

    fn speaker_of(f: &SegmentsFile) -> Vec<Option<String>> {
        f.segments.iter().map(|s| s.speaker_id.clone()).collect()
    }

    #[test]
    fn ids_labels_and_colours() {
        assert_eq!(voice_id(3), "voice:3");
        assert_eq!(voice_number("voice:12"), Some(12));
        for bad in [
            "voice:0",
            "voice:",
            "voice:x",
            "you",
            "meet:Anna",
            NEW_VOICE,
        ] {
            assert_eq!(voice_number(bad), None, "{bad}");
        }
        let v = voice_speaker(2);
        assert_eq!((v.label.as_str(), v.color.as_str()), ("Voice 2", "#7e22ce"));
        assert_eq!(voice_color(9), voice_color(1));
        assert_eq!(you_speaker().id, "you");
    }

    #[test]
    fn redetect_labels_a_document_without_voices() {
        let truth = [0, 1, 0, 2, 1, 2, 0, 1, 2, 0];
        let mut f = doc(&truth, &[None; 10], 5);
        let r = redetect(&mut f).unwrap();
        assert_eq!(
            r,
            Redetected {
                voices: 3,
                changed: 10
            }
        );
        let want: Vec<Option<String>> = truth
            .iter()
            .map(|t| Some(voice_id(*t as u32 + 1)))
            .collect();
        assert_eq!(speaker_of(&f), want);
        let ids: Vec<&str> = f.speakers.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["voice:1", "voice:2", "voice:3"]);
    }

    #[test]
    fn redetect_is_idempotent_and_keeps_renamed_voices() {
        let truth = [0, 1, 0, 1, 1, 0, 0, 1];
        // Live labels over-split speaker 1 into voices 2 and 3; voice 2
        // was renamed "Anna" by the user.
        let live = [
            Some(1),
            Some(2),
            Some(1),
            Some(3),
            Some(2),
            Some(1),
            Some(1),
            Some(3),
        ];
        let mut f = doc(&truth, &live, 9);
        rename_speaker(&mut f, "voice:2", "Anna").unwrap();
        let r = redetect(&mut f).unwrap();
        assert_eq!(r.voices, 2);
        assert_eq!(r.changed, 2);
        let want: Vec<Option<String>> = truth
            .iter()
            .map(|t| Some(if *t == 0 { "voice:1" } else { "voice:2" }.to_string()))
            .collect();
        assert_eq!(speaker_of(&f), want);
        // Voice 3 is gone, "Anna" kept her name.
        assert_eq!(f.speakers.len(), 2);
        assert_eq!(f.speakers[1].label, "Anna");

        let once = f.clone();
        let again = redetect(&mut f).unwrap();
        assert_eq!(again.changed, 0);
        assert_eq!(f, once);
    }

    #[test]
    fn redetect_gives_new_clusters_fresh_numbers() {
        // Live saw only one voice for two people.
        let truth = [0, 1, 0, 1, 0, 1];
        let mut f = doc(&truth, &[Some(4); 6], 21);
        redetect(&mut f).unwrap();
        let got = speaker_of(&f);
        // The cluster sharing more speech keeps voice:4, the other is new
        // (numbered after the highest voice ever seen).
        let ids: std::collections::BTreeSet<_> = got.iter().flatten().cloned().collect();
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), ["voice:4", "voice:5"]);
        assert_ne!(got[0], got[1]);
        assert_eq!(got[0], got[2]);
    }

    #[test]
    fn redetect_leaves_you_meet_names_and_lines_without_data_alone() {
        let (mut rng, v) = voices(2, 4);
        let mut f = SegmentsFile {
            speakers: vec![you_speaker()],
            segments: vec![
                seg(0, 0, 4000, Some("you"), Some(sample(&mut rng, &v[0], 0.9))),
                seg(1, 5000, 4000, None, Some(sample(&mut rng, &v[1], 0.9))),
                seg(2, 10000, 500, None, None),
                seg(
                    3,
                    11000,
                    4000,
                    Some("meet:Anna"),
                    Some(sample(&mut rng, &v[1], 0.9)),
                ),
            ],
            ..Default::default()
        };
        redetect(&mut f).unwrap();
        assert_eq!(
            speaker_of(&f),
            [
                Some("you".into()),
                Some("voice:1".into()),
                None,
                Some("meet:Anna".into())
            ]
        );
        assert_eq!(f.speakers[0], you_speaker());

        let mut empty = SegmentsFile {
            segments: vec![seg(0, 0, 4000, None, None)],
            ..Default::default()
        };
        assert!(redetect(&mut empty)
            .unwrap_err()
            .to_string()
            .contains("no voice data"));
    }

    #[test]
    fn redetect_keeps_overlapped_lines_and_gives_them_a_second_voice() {
        use crate::archive::OverlapSpan;
        let truth = [0, 1, 0, 1, 0, 1];
        let mut f = doc(&truth, &[None; 6], 5);
        // Line 2 (speaker 0, 10–14 s) overlaps at its end with line 3's
        // speaker; stored before any voice existed.
        f.segments[2].overlap = vec![OverlapSpan {
            start_ms: 13_000,
            end_ms: 14_000,
            speaker_id: None,
        }];
        redetect(&mut f).unwrap();
        // The overlapped line is clustered like the others…
        assert_eq!(f.segments[2].speaker_id, f.segments[0].speaker_id);
        // …and its span names the nearest other voice.
        assert_eq!(
            f.segments[2].overlap[0].speaker_id,
            f.segments[3].speaker_id
        );
        assert_ne!(
            f.segments[2].overlap[0].speaker_id,
            f.segments[2].speaker_id
        );
        // Idempotent with the spans too.
        let once = f.clone();
        assert_eq!(redetect(&mut f).unwrap().changed, 0);
        assert_eq!(f, once);
    }

    #[test]
    fn move_line_to_an_existing_or_a_new_voice() {
        let mut f = doc(&[0, 1, 0], &[Some(1), Some(2), Some(1)], 2);
        assert_eq!(move_segment(&mut f, 2, "voice:2").unwrap(), "voice:2");
        assert_eq!(f.segments[2].speaker_id.as_deref(), Some("voice:2"));
        assert_eq!(move_segment(&mut f, 0, NEW_VOICE).unwrap(), "voice:3");
        assert_eq!(f.segments[0].speaker_id.as_deref(), Some("voice:3"));
        assert_eq!(f.speakers.last().unwrap(), &voice_speaker(3));
        // Voice 1 has no line left but stays listed (the user may move a
        // line back); only Re-detect prunes.
        assert!(f.speakers.iter().any(|s| s.id == "voice:1"));
        assert!(move_segment(&mut f, 0, "voice:9").is_err());
        assert!(move_segment(&mut f, 42, "voice:1").is_err());
    }

    #[test]
    fn rename_is_per_speaker_trimmed_and_resettable() {
        let mut f = doc(&[0, 1], &[Some(1), Some(2)], 2);
        f.speakers.push(you_speaker());
        rename_speaker(&mut f, "voice:1", "  Anna \n Rossi ").unwrap();
        assert_eq!(f.speakers[0].label, "Anna Rossi");
        assert_eq!(f.speakers[1].label, "Voice 2");
        rename_speaker(&mut f, "voice:1", "  ").unwrap();
        assert_eq!(f.speakers[0].label, "Voice 1");
        assert!(rename_speaker(&mut f, "you", "").is_err());
        rename_speaker(&mut f, "you", "Francesco").unwrap();
        assert!(rename_speaker(&mut f, "voice:7", "X").is_err());
        assert!(rename_speaker(&mut f, "voice:1", &"x".repeat(61)).is_err());
    }

    fn anna() -> Person {
        Person {
            id: "p-anna".into(),
            name: "Anna Rossi".into(),
            email: Some("anna@example.com".into()),
            aliases: vec!["Anna R.".into()],
        }
    }

    fn meeting_meta(participants: Vec<Participant>) -> ItemMeta {
        ItemMeta {
            item_type: crate::archive::ItemType::Meeting,
            participants,
            ..Default::default()
        }
    }

    fn part(name: &str, email: Option<&str>) -> Participant {
        Participant {
            name: name.into(),
            email: email.map(Into::into),
        }
    }

    #[test]
    fn linking_a_voice_names_it_and_adds_the_participant() {
        let mut f = doc(&[0, 1], &[Some(1), Some(2)], 2);
        let mut meta = meeting_meta(vec![part("Marco", None)]);
        link_speaker(&mut f, &mut meta, "voice:2", &anna()).unwrap();
        let sp = &f.speakers[1];
        assert_eq!(sp.person_id.as_deref(), Some("p-anna"));
        assert_eq!(sp.label, "Anna Rossi");
        assert_eq!(sp.label_before_link.as_deref(), Some("Voice 2"));
        assert_eq!(
            meta.participants,
            vec![
                part("Marco", None),
                part("Anna Rossi", Some("anna@example.com"))
            ]
        );
        // Linking again changes nothing (no duplicate participant).
        let (f1, m1) = (f.clone(), meta.clone());
        link_speaker(&mut f, &mut meta, "voice:2", &anna()).unwrap();
        assert_eq!((f, meta), (f1, m1));
    }

    #[test]
    fn unlink_restores_the_previous_label_and_keeps_participants() {
        let mut f = doc(&[0, 1], &[Some(1), Some(2)], 2);
        let mut meta = meeting_meta(vec![]);
        link_speaker(&mut f, &mut meta, "voice:1", &anna()).unwrap();
        unlink_speaker(&mut f, "voice:1").unwrap();
        assert_eq!(f.speakers[0], voice_speaker(1));
        assert_eq!(meta.participants.len(), 1);
        assert!(unlink_speaker(&mut f, "voice:9").is_err());
    }

    #[test]
    fn a_renamed_voice_keeps_its_name_when_linked() {
        let mut f = doc(&[0, 1], &[Some(1), Some(2)], 2);
        rename_speaker(&mut f, "voice:1", "Anna").unwrap();
        let mut meta = meeting_meta(vec![]);
        link_speaker(&mut f, &mut meta, "voice:1", &anna()).unwrap();
        assert_eq!(f.speakers[0].label, "Anna");
        assert_eq!(f.speakers[0].label_before_link, None);
        unlink_speaker(&mut f, "voice:1").unwrap();
        assert_eq!(f.speakers[0].label, "Anna");
        // Renaming after a link: the new name is what unlink keeps.
        link_speaker(&mut f, &mut meta, "voice:2", &anna()).unwrap();
        rename_speaker(&mut f, "voice:2", "Annina").unwrap();
        unlink_speaker(&mut f, "voice:2").unwrap();
        assert_eq!(f.speakers[1].label, "Annina");
    }

    #[test]
    fn linking_completes_an_existing_participant_without_replacing_emails() {
        let mut f = doc(&[0], &[Some(2)], 2);
        // "Voice 2" typed as a participant becomes the person.
        let mut meta = meeting_meta(vec![part("Voice 2", None)]);
        link_speaker(&mut f, &mut meta, "voice:2", &anna()).unwrap();
        assert_eq!(
            meta.participants,
            vec![part("Anna Rossi", Some("anna@example.com"))]
        );
        // An alias with its own email: completed by name, email kept.
        let mut f = doc(&[0], &[Some(1)], 2);
        let mut meta = meeting_meta(vec![part("anna r.", Some("a.rossi@work.example"))]);
        link_speaker(&mut f, &mut meta, "voice:1", &anna()).unwrap();
        assert_eq!(
            meta.participants,
            vec![part("anna r.", Some("a.rossi@work.example"))]
        );
        // A note never gets participants.
        let mut note = ItemMeta::default();
        link_speaker(&mut f, &mut note, "voice:1", &anna()).unwrap();
        assert!(note.participants.is_empty());
    }

    #[test]
    fn meet_names_and_renames_matching_a_person_are_suggested() {
        let mut f = doc(&[0, 1], &[Some(1), Some(2)], 2);
        f.speakers.push(DocSpeaker {
            id: "meet:Anna R.".into(),
            label: "Anna R.".into(),
            ..Default::default()
        });
        let people = vec![anna()];
        let ids = |f: &SegmentsFile| -> Vec<String> {
            link_suggestions(f, &people)
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        };
        assert_eq!(ids(&f), ["meet:Anna R."]);
        rename_speaker(&mut f, "voice:1", "anna rossi").unwrap();
        assert_eq!(ids(&f), ["voice:1", "meet:Anna R."]);
        let mut meta = meeting_meta(vec![]);
        link_speaker(&mut f, &mut meta, "voice:1", &anna()).unwrap();
        assert_eq!(ids(&f), ["meet:Anna R."]);
        // A Meet name's default label is the name itself.
        assert_eq!(default_label("meet:Anna R.").as_deref(), Some("Anna R."));
        assert!(!renamed(&f.speakers[2]));
    }

    #[test]
    fn finalize_live_folds_strays_and_compacts_numbers() {
        // Voice 2 is a 4 s stray of speaker 0; voices 1 and 3 are real.
        let truth = [0, 1, 0, 0, 1, 1, 0];
        let live = [
            Some(1),
            Some(3),
            Some(1),
            Some(2),
            Some(3),
            Some(3),
            Some(1),
        ];
        let mut f = doc(&truth, &live, 13);
        finalize_live(&mut f);
        let want: Vec<Option<String>> = truth
            .iter()
            .map(|t| Some(voice_id(*t as u32 + 1)))
            .collect();
        assert_eq!(speaker_of(&f), want);
        let ids: Vec<&str> = f.speakers.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["voice:1", "voice:2"]);
        assert_eq!(f.speakers[1].color, voice_color(2));
        // Already tidy: untouched.
        let once = f.clone();
        finalize_live(&mut f);
        assert_eq!(f, once);
    }
}
