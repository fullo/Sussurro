//! End-to-end fixtures: the #236 bake-off text set (the same ten texts the
//! listening test used, Italian and English) and archive-style markdown
//! documents, through [`prepare`] with the default options.

use super::*;

const IT: Lang = Lang::Italian;
const EN: Lang = Lang::English;

fn speak(doc: &str, lang: Lang) -> String {
    speakable_text(doc, lang, &PrepOptions::default())
}

/// The bake-off texts, as plain text.
const BAKEOFF: &[(&str, Lang, &str, &str)] = &[
    (
        "en-1-numbers",
        EN,
        include_str!("../testdata/en-1-numbers.txt"),
        "The quote comes to one thousand two hundred forty-eight dollars and fifty cents, \
         including twenty-two percent tax. Flight B A two four nine zero departs at seven \
         forty-five ay em from Terminal five, the booking code is X four K nine Q two. Please \
         call plus four four, two zero, seven nine four six, zero nine five eight before six \
         pee em.",
    ),
    (
        "en-2-dates",
        EN,
        include_str!("../testdata/en-2-dates.txt"),
        "The meeting moved from Monday, March third, twenty twenty-five to Thursday, April \
         seventeenth. The contract expires on December thirty-first, twenty twenty-six, and \
         the first payment is due by January fifteenth. It was the nineteenth century, \
         eighteen sixty-one to be exact.",
    ),
    (
        "en-3-names",
        EN,
        include_str!("../testdata/en-3-names.txt"),
        "The panel included Siobhan Nguyen, Wojciech Szczęsny and Chimamanda Ngozi Adichie, \
         followed by Joaquin Phoenix and Nicolás Maduro. They met in Worcester, then \
         Loughborough, and finally Albuquerque.",
    ),
    (
        "en-4-news",
        EN,
        include_str!("../testdata/en-4-news.txt"),
        "London, September twenty-fourth. After three days of talks, the city council \
         approved its new plan for sustainable transport. The project adds forty kilometres \
         of cycle lanes by twenty twenty-eight and a thirty per cent increase in night bus \
         services. The opposition criticised the timeline as too slow, while cycling groups \
         welcomed the news with cautious optimism. \"It's a first step,\" a spokeswoman said, \
         \"but now we need the money.\"",
    ),
    (
        "en-5-transcript",
        EN,
        include_str!("../testdata/en-5-transcript.txt"),
        "Mark: So, let's start with the budget. We're about ten per cent under. Julia: Yes, \
         but only because the September invoices haven't arrived yet. Mark: Fine. Who's \
         handling it? Julia: I'll handle it, I'll send you everything by Friday.",
    ),
    (
        "it-1-numbers",
        IT,
        include_str!("../testdata/it-1-numbers.txt"),
        "Il preventivo è di milleduecentoquarantotto euro e cinquanta centesimi, IVA al \
         ventidue per cento inclusa. Il volo A Z uno sette sette quattro parte alle sette e \
         quarantacinque dal terminal tre, il codice di prenotazione è X quattro K nove Q due. \
         Chiamate il numero zero due, otto tre nove quattro, uno uno sette zero entro le \
         diciotto.",
    ),
    (
        "it-2-dates",
        IT,
        include_str!("../testdata/it-2-dates.txt"),
        "La riunione è stata spostata da lunedì tre marzo duemilaventicinque a giovedì \
         diciassette aprile. Il contratto scade il trentuno dicembre duemilaventisei e la \
         prima rata va pagata entro il quindici gennaio. Era il diciannovesimo secolo, \
         precisamente il milleottocentosessantuno.",
    ),
    (
        "it-3-names",
        IT,
        include_str!("../testdata/it-3-names.txt"),
        "Ne hanno parlato Giuseppe Ungaretti, Wisława Szymborska e Francesco Guccini, poi \
         Chiara Ferragni e Jürgen Habermas. Il gruppo si è ritrovato a Reggio Emilia, poi a \
         Castelnuovo di Garfagnana e infine a Bressanone.",
    ),
    (
        "it-4-news",
        IT,
        include_str!("../testdata/it-4-news.txt"),
        "Roma, ventiquattro settembre. Dopo tre giorni di trattative, il consiglio comunale \
         ha approvato il nuovo piano per la mobilità sostenibile. Il progetto prevede \
         quaranta chilometri di piste ciclabili entro il duemilaventotto e un aumento del \
         trenta per cento delle corse notturne degli autobus. L'opposizione ha criticato i \
         tempi, giudicati troppo lunghi, mentre le associazioni dei ciclisti hanno accolto la \
         notizia con cauto ottimismo. «È un primo passo», ha detto la portavoce, «ma adesso \
         servono i fondi».",
    ),
    (
        "it-5-transcript",
        IT,
        include_str!("../testdata/it-5-transcript.txt"),
        "Marco: Allora, partiamo dal budget. Siamo sotto di circa il dieci per cento. Giulia: \
         Sì, ma solo perché le fatture di settembre non sono ancora arrivate. Marco: Va bene. \
         Chi se ne occupa? Giulia: Me ne occupo io, ti mando tutto entro venerdì.",
    ),
];

#[test]
fn bakeoff_texts_become_speakable() {
    for (name, lang, input, want) in BAKEOFF {
        assert_eq!(speak(input, *lang), *want, "{name}");
    }
}

#[test]
fn bakeoff_texts_have_no_digits_or_symbols_left() {
    for (name, lang, input, _) in BAKEOFF {
        let out = speak(input, *lang);
        assert!(
            !out.chars()
                .any(|c| c.is_ascii_digit() || "%$€£&#@/;".contains(c)),
            "{name}: {out}"
        );
    }
}

#[test]
fn bakeoff_chunks_respect_the_default_limit() {
    for (name, lang, input, _) in BAKEOFF {
        let chunks = prepare(input, *lang, &PrepOptions::default());
        assert!(!chunks.is_empty(), "{name}");
        for c in &chunks {
            assert!(
                c.text.chars().count() <= DEFAULT_MAX_CHUNK_CHARS,
                "{name}: {} chars: {}",
                c.text.chars().count(),
                c.text
            );
        }
    }
}

#[test]
fn italian_note_document() {
    let doc = include_str!("../testdata/it-note.md");
    assert_eq!(
        speak(doc, IT),
        "Viaggio a Bologna.\n\
         Il treno parte alle sette e quarantacinque dal binario tre. Costa ventinove euro e \
         novanta centesimi a persona, cioè il quindici per cento in meno.\n\
         Da fare.\n\
         Prenotare l'albergo, vedi sito.\n\
         Chiamare il dottor Rossi al numero zero cinque uno, uno due tre, quattro cinque sei \
         sette.\n\
         Comprare i biglietti.\n\
         Orari.\n\
         Treno: F R nove cinque zero tre, Partenza: sette e quarantacinque, Arrivo: nove e due.\n\
         Treno: I C cinque otto otto, Partenza: otto e dieci, Arrivo: dieci e trentacinque.\n\
         Nota: la riunione è il primo ottobre duemilaventisei, ore quattordici e trenta."
    );
}

#[test]
fn italian_note_pauses() {
    let doc = include_str!("../testdata/it-note.md");
    let chunks = prepare(doc, IT, &PrepOptions::default());
    let pauses: Vec<Pause> = chunks.iter().map(|c| c.pause_after).collect();
    assert_eq!(
        pauses,
        [
            Pause::Section, // # Viaggio a Bologna
            Pause::Section, // paragraph before a heading
            Pause::Section, // ## Da fare
            Pause::Line,    // list
            Pause::Line,
            Pause::Section,   // last item, before a heading
            Pause::Section,   // ## Orari
            Pause::Line,      // table row
            Pause::Paragraph, // last row
            Pause::Paragraph, // Nota
        ]
    );
    let indexes: Vec<usize> = chunks.iter().map(|c| c.index).collect();
    assert_eq!(indexes, (0..chunks.len()).collect::<Vec<_>>());
}

#[test]
fn italian_note_without_tables() {
    let doc = include_str!("../testdata/it-note.md");
    let opts = PrepOptions {
        tables: TableMode::Skip,
        ..Default::default()
    };
    let text = speakable_text(doc, IT, &opts);
    assert!(!text.contains("Treno:"), "{text}");
    assert!(text.contains("Orari.\nNota:"), "{text}");
}

#[test]
fn italian_meeting_transcript() {
    let doc = include_str!("../testdata/it-meeting.md");
    assert_eq!(
        speak(doc, IT),
        "Riunione budget.\n\
         Marco: Allora, partiamo dal budget: siamo al dieci per cento sotto.\n\
         Il totale è di dodicimilacinquecento euro.\n\
         Giulia: Sì, ma le fatture di settembre arrivano il trenta settembre.\n\
         Nessuno ha obiezioni."
    );
    let dropped = speakable_text(
        doc,
        IT,
        &PrepOptions {
            speakers: SpeakerMode::Drop,
            ..Default::default()
        },
    );
    assert!(
        !dropped.contains("Marco") && !dropped.contains("Giulia"),
        "{dropped}"
    );
    assert!(!dropped.contains("00:"), "{dropped}");
}

#[test]
fn english_news_document() {
    let doc = include_str!("../testdata/en-news.md");
    assert_eq!(
        speak(doc, EN),
        "Council approves transport plan.\n\
         London, September twenty-fourth, twenty twenty-six, After three days of talks, the \
         council approved a one point two billion pounds plan, see the report.\n\
         Key figures.\n\
         Forty kilometers of new cycle lanes by twenty twenty-eight.\n\
         Night buses up thirty percent, from eleven thirty pee em to five ay em.\n\
         Fares frozen at two pounds and fifty pence until March thirty-first, twenty \
         twenty-seven.\n\
         Line: N twenty-nine, Frequency: every twelve minutes, Cost: one pound and seventy-five \
         pence.\n\
         Doctor Jane O'Brien, the council's transport lead, said: \"It's the first step, not the \
         last.\""
    );
}

#[test]
fn chunks_are_cut_to_the_limit_and_keep_all_words() {
    let doc = include_str!("../testdata/it-4-news.txt");
    for max in [MIN_MAX_CHUNK_CHARS, 60, 100, DEFAULT_MAX_CHUNK_CHARS, 1000] {
        let opts = PrepOptions {
            max_chunk_chars: max,
            ..Default::default()
        };
        let chunks = prepare(doc, IT, &opts);
        assert!(
            chunks.iter().all(|c| c.text.chars().count() <= max),
            "{max}"
        );
        let joined: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        assert_eq!(
            joined.join(" "),
            normalize_text(doc, IT),
            "max {max}: no word lost or added"
        );
        // Mid-sentence cuts continue without a pause; the last chunk takes
        // the paragraph's.
        assert_eq!(chunks.last().map(|c| c.pause_after), Some(Pause::Paragraph));
    }
}

#[test]
fn the_default_chunking_of_the_news_text() {
    let chunks = prepare(
        include_str!("../testdata/en-4-news.txt"),
        EN,
        &PrepOptions::default(),
    );
    let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "London, September twenty-fourth. After three days of talks, the city council \
             approved its new plan for sustainable transport.",
            "The project adds forty kilometres of cycle lanes by twenty twenty-eight and a thirty \
             per cent increase in night bus services.",
            "The opposition criticised the timeline as too slow, while cycling groups welcomed \
             the news with cautious optimism.",
            "\"It's a first step,\" a spokeswoman said, \"but now we need the money.\"",
        ]
    );
}

#[test]
fn a_tiny_limit_is_raised_to_the_minimum() {
    let opts = PrepOptions {
        max_chunk_chars: 1,
        ..Default::default()
    };
    let chunks = prepare(
        "A short sentence that is longer than forty characters in total.",
        EN,
        &opts,
    );
    assert!(chunks.len() >= 2);
    assert!(chunks
        .iter()
        .all(|c| c.text.chars().count() <= MIN_MAX_CHUNK_CHARS));
    assert_eq!(chunks[0].pause_after, Pause::None);
}

#[test]
fn same_document_same_chunks() {
    let doc = include_str!("../testdata/en-news.md");
    let a = prepare(doc, EN, &PrepOptions::default());
    let b = prepare(doc, EN, &PrepOptions::default());
    assert_eq!(a, b);
}

#[test]
fn other_languages_keep_numbers_but_lose_markup() {
    let doc = "---\nlanguage: fr\n---\n# Le **plan**\n\nLe 3 mars 2025, voir [le site](https://www.exemple.fr/x) 🎉.";
    assert_eq!(
        speakable_text(doc, Lang::from_code("fr"), &PrepOptions::default()),
        "Le plan.\nLe 3 mars 2025, voir le site."
    );
}

#[test]
fn language_codes() {
    for code in ["it", "IT", "it-IT", "it_CH", "ita", "Italiano"] {
        assert_eq!(Lang::from_code(code), IT, "{code}");
    }
    for code in ["en", "en-GB", "EN_us", "eng", "English"] {
        assert_eq!(Lang::from_code(code), EN, "{code}");
    }
    for code in ["", "fr", "de-DE", "auto", "i"] {
        assert_eq!(Lang::from_code(code), Lang::Other, "{code}");
    }
}

#[test]
fn empty_documents_give_no_chunks() {
    assert!(prepare("", IT, &PrepOptions::default()).is_empty());
    assert!(prepare(
        "---\ntitle: x\n---\n\n```\ncode\n```\n",
        EN,
        &PrepOptions::default()
    )
    .is_empty());
}

#[test]
fn pause_order_and_suggestions() {
    assert!(Pause::None < Pause::Sentence && Pause::Paragraph < Pause::Section);
    assert_eq!(Pause::None.suggested_ms(), 0);
    assert!(Pause::Sentence.suggested_ms() < Pause::Line.suggested_ms());
    assert!(Pause::Paragraph.suggested_ms() < Pause::Section.suggested_ms());
}
