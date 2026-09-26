//! Live checks of the read-aloud engine on the real pinned models
//! (`#[ignore]`: they need ~1.7 GB of downloaded files and minutes of CPU).
//!
//! ```text
//! SUSSURRO_TTS_MODELS=<models folder holding pocket-tts/…>   (required)
//! SUSSURRO_TTS_OUT=<folder for the WAVs>                      (default: temp dir)
//! SUSSURRO_TTS_LANG=it|en|both                                (default: it)
//! SUSSURRO_TTS_THREADS=2                                      (intra-op threads)
//! SUSSURRO_TTS_TOKENS_IT / _EN=<json {"text": [ids…]} from sentencepiece> (optional)
//! SUSSURRO_TTS_WHISPER=<ggml whisper model> (optional: transcribe the speech back)
//! cargo test --lib live_tts -- --ignored --nocapture
//! ```
//!
//! The models folder can be filled by the app (Models → Voices) or by
//! hand from the pinned URLs in [`super::catalog`]; files are checked
//! against their pinned sizes here and never downloaded by the test.

use super::catalog::{self, Language};
use super::engine;
use super::models;
use super::pocket::tokenizer::Tokenizer;
use super::pocket::{PocketOptions, PocketTts};
use super::text::{self, Lang, PrepOptions};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn models_dir() -> PathBuf {
    PathBuf::from(std::env::var("SUSSURRO_TTS_MODELS").expect("set SUSSURRO_TTS_MODELS"))
}

fn languages() -> Vec<&'static Language> {
    match std::env::var("SUSSURRO_TTS_LANG").as_deref() {
        Ok("en") => vec![&catalog::ENGLISH],
        Ok("both") => vec![&catalog::ITALIAN, &catalog::ENGLISH],
        _ => vec![&catalog::ITALIAN],
    }
}

/// Ids from `sentencepiece` 0.2 on the pinned tokenizers.
const REFERENCE_IDS: &[(&str, &str, &[u32])] = &[
    (
        "it",
        "Ciao mondo. Sono il Pocket TTS di Kyutai.",
        &[
            3638, 486, 263, 1097, 273, 1076, 792, 1452, 260, 926, 926, 945, 262, 260, 79, 472,
            1874, 281, 263,
        ],
    ),
    (
        "it",
        "Perché è così? Ünïcödé — “quotes” … 3,5%",
        &[
            260, 601, 271, 383, 316, 260, 199, 160, 352, 199, 179, 408, 199, 186, 590, 2512, 260,
            230, 132, 152, 260, 230, 132, 160, 3998, 414, 285, 287, 325, 230, 132, 161, 260, 230,
            132, 170, 260, 456, 261, 443, 41,
        ],
    ),
    (
        "it",
        "  double  space\tand tab",
        &[
            260, 260, 462, 414, 674, 320, 260, 1378, 402, 13, 279, 1064, 260, 289, 674,
        ],
    ),
    (
        "en",
        "Hello world, this is a test!",
        &[2994, 578, 262, 285, 277, 267, 1115, 682],
    ),
    (
        "en",
        "Perché è così? Ünïcödé — “quotes” … 3,5%",
        &[
            2927, 504, 745, 260, 3208, 571, 261, 199, 176, 292, 260, 199, 160, 306, 199, 179, 440,
            2451, 307, 745, 260, 3133, 260, 230, 132, 160, 1431, 1025, 371, 230, 132, 161, 260,
            230, 132, 170, 260, 450, 262, 437, 885,
        ],
    ),
];

#[test]
#[ignore = "needs the downloaded read-aloud models (SUSSURRO_TTS_MODELS)"]
fn live_tts_tokenizer_matches_sentencepiece() {
    let dir = models_dir();
    let mut checked = 0;
    for l in catalog::LANGUAGES {
        let path = models::language_dir(&dir, l).join("tokenizer.model");
        let Ok(tok) = Tokenizer::from_file(&path) else {
            continue;
        };
        assert_eq!(tok.vocab_size(), 4000);
        for (code, text, ids) in REFERENCE_IDS {
            if *code == l.code {
                assert_eq!(tok.encode(text), *ids, "{code}: {text:?}");
                checked += 1;
            }
        }
        // Optional bigger reference set: {"<text>": [ids…]} per language.
        if let Ok(p) = std::env::var(format!("SUSSURRO_TTS_TOKENS_{}", l.code.to_uppercase())) {
            let refs: std::collections::BTreeMap<String, Vec<u32>> =
                serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
            for (text, ids) in &refs {
                assert_eq!(&tok.encode(text), ids, "{}: {text:?}", l.code);
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no tokenizer found under {}", dir.display());
    eprintln!("tokenizer: {checked} texts match sentencepiece");
}

#[test]
#[ignore = "needs the downloaded read-aloud models (SUSSURRO_TTS_MODELS); minutes of CPU"]
fn live_tts_renders_the_fixture_text() {
    let dir = models_dir();
    let out_dir = std::env::var("SUSSURRO_TTS_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let threads = std::env::var("SUSSURRO_TTS_THREADS")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(2);
    for l in languages() {
        assert!(models::model_present(&dir, l), "{} model missing", l.label);
        let voice = l.voice(l.default_voice).unwrap();
        assert!(
            models::voice_present(&dir, l, voice),
            "{} missing",
            voice.id
        );
        let fixture = format!("src/tts/testdata/{}-4-news.txt", l.code);
        let source = std::fs::read_to_string(&fixture).unwrap();
        let chunks = text::prepare(&source, Lang::from_code(l.code), &PrepOptions::default());
        assert!(!chunks.is_empty());

        let started = Instant::now();
        let mut tts = PocketTts::load(
            &models::language_dir(&dir, l),
            &models::voice_path(&dir, l, voice),
            PocketOptions {
                threads,
                ..PocketOptions::default()
            },
        )
        .unwrap();
        let load_s = started.elapsed().as_secs_f32();

        let started = Instant::now();
        let mut first_audio = None;
        let mut audio: Vec<f32> = Vec::new();
        let stats = engine::render(
            &mut tts,
            &chunks,
            &AtomicBool::new(false),
            &mut |_, _| {},
            &mut |pcm| {
                first_audio.get_or_insert_with(|| started.elapsed().as_secs_f32());
                audio.extend_from_slice(pcm);
                Ok(())
            },
        )
        .unwrap();
        let compute_s = started.elapsed().as_secs_f32();
        let rate = engine::TtsEngine::sample_rate(&tts);
        let audio_s = audio.len() as f32 / rate as f32;
        let out = out_dir.join(format!("live-tts-{}-{}.wav", l.code, voice.id));
        engine::write_wav(&out, rate, &audio).unwrap();

        let rms = (audio.iter().map(|x| x * x).sum::<f32>() / audio.len() as f32).sqrt();
        let peak = audio.iter().fold(0f32, |m, x| m.max(x.abs()));
        eprintln!(
            "{}: {} chunks, load {load_s:.1} s, first audio {:.2} s, {compute_s:.1} s for {audio_s:.1} s of audio (RTF {:.2}), rms {rms:.3}, peak {peak:.2} → {}",
            l.label,
            stats.chunks,
            first_audio.unwrap_or(0.0),
            compute_s / audio_s,
            out.display()
        );
        assert_eq!(rate, 24_000);
        assert!(audio.iter().all(|x| x.is_finite()));
        // Roughly 12–20 characters per second of speech: a text of this
        // size that comes out far shorter lost sentences (the EOS-at-step-1
        // failure of mismatched revisions), far longer ran without an end.
        let chars = source.chars().count() as f32;
        assert!(
            audio_s > chars / 30.0,
            "too short: {audio_s:.1} s for {chars} chars"
        );
        assert!(
            audio_s < chars / 6.0,
            "too long: {audio_s:.1} s for {chars} chars"
        );
        assert!(rms > 0.01, "silent output (rms {rms})");

        // Optional round trip through Whisper: the speech must say the text.
        if let Ok(model) = std::env::var("SUSSURRO_TTS_WHISPER") {
            let stt = crate::stt::whisper::Transcriber::load(std::path::Path::new(&model)).unwrap();
            let pcm16 = crate::audio::resample::resample_linear(&audio, rate, 16_000);
            let heard = stt.transcribe(&pcm16, None, l.code).unwrap();
            let said =
                text::speakable_text(&source, Lang::from_code(l.code), &PrepOptions::default());
            let recall = word_recall(&said, &heard);
            eprintln!(
                "{}: Whisper heard ({:.0}% of the words): {heard}",
                l.label,
                recall * 100.0
            );
            assert!(
                recall > 0.7,
                "the speech doesn't say the text (recall {recall:.2})"
            );
        }
    }
}

/// Share of `said`'s words (lowercase, letters only) that `heard` has.
fn word_recall(said: &str, heard: &str) -> f32 {
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let heard: std::collections::HashSet<String> = words(heard).into_iter().collect();
    let said = words(said);
    said.iter().filter(|w| heard.contains(*w)).count() as f32 / said.len().max(1) as f32
}

/// Read aloud end to end (#256): a note in a temporary archive becomes a
/// saved, marked `speech.opus` through the real engine, and the Opus file
/// decodes through the scheme's reader. Uses the first language of
/// `SUSSURRO_TTS_LANG` and a short text, so it stays under a minute.
#[test]
#[ignore = "needs the downloaded read-aloud models (SUSSURRO_TTS_MODELS)"]
fn live_read_aloud_saves_a_marked_speech_file() {
    use super::read_aloud::{self, Jobs, PocketSpeaker, Request, Target};
    let dir = models_dir();
    let l = languages()[0];
    let threads = std::env::var("SUSSURRO_TTS_THREADS")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(2);
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path();
    let body = if l.code == "it" {
        "# Promemoria\n\nDomani alle 9:30 chiamo Anna per il budget di 1.200 euro.\n"
    } else {
        "# Reminder\n\nTomorrow at 9:30 I call Anna about the $1,200 budget.\n"
    };
    let meta = crate::archive::ItemMeta {
        title: "Read me".into(),
        date: "2026-09-26T10:00:00+02:00".into(),
        language: l.code.into(),
        ..Default::default()
    };
    let id = crate::archive::create_item(archive, &meta, &Default::default()).unwrap();
    let item_dir = crate::archive::store::existing_item_dir(archive, &id).unwrap();
    std::fs::write(
        item_dir.join("transcript.md"),
        format!("---\ntype: note\ntitle: Read me\nlanguage: {}\n---\n{body}", l.code),
    )
    .unwrap();
    let speaker = PocketSpeaker {
        service: super::service::global(),
        models_dir: &dir,
        options: PocketOptions {
            threads,
            ..PocketOptions::default()
        },
    };
    let voices = std::collections::BTreeMap::new();
    let started = Instant::now();
    let out = read_aloud::run(
        &Jobs::default(),
        &speaker,
        &Request {
            archive,
            id: &id,
            document: None,
            language: None,
            voices: &voices,
            target: Target::Save,
        },
        &mut |s| eprintln!("read aloud: {}/{}", s.done, s.total),
    )
    .unwrap();
    let path = item_dir.join(&out.file);
    eprintln!(
        "{}: {:.1} s of speech in {:.1} s → {} ({} bytes)",
        l.label,
        out.seconds,
        started.elapsed().as_secs_f32(),
        path.display(),
        std::fs::metadata(&path).unwrap().len()
    );
    assert_eq!(out.file, "speech.opus");
    assert!(out.seconds > 2.0 && out.seconds < 30.0, "{}", out.seconds);
    assert!(crate::archive::opus::verify(&path).unwrap() > 0);
    let tags = crate::archive::opus::read_tags(&path).unwrap();
    assert!(tags.contains(&("SYNTHETIC".into(), "1".into())), "{tags:?}");
    let st = read_aloud::statuses(archive, &id).unwrap();
    assert!(st[0].recorded && !st[0].stale);
    if let Ok(keep) = std::env::var("SUSSURRO_TTS_OUT") {
        let to = PathBuf::from(keep).join(format!("live-read-aloud-{}.opus", l.code));
        let _ = std::fs::copy(&path, to);
    }
}
