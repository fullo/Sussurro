use std::sync::OnceLock;

/// Deterministic spoken editing commands, applied without the LLM (so they
/// work even with Cleanup None). Contextual commands like "scratch that" are
/// handled by the LLM via the cleanup prompt instead.
///
/// A command is matched as a whole phrase, case-insensitively, swallowing the
/// punctuation whisper tends to wrap around it ("First. New line. Second"
/// becomes "First.\nSecond").
pub fn apply_basic_commands(text: &str) -> String {
    static PARAGRAPH: OnceLock<regex::Regex> = OnceLock::new();
    static FULL_STOP: OnceLock<regex::Regex> = OnceLock::new();
    static BULLET: OnceLock<regex::Regex> = OnceLock::new();
    static LINE: OnceLock<regex::Regex> = OnceLock::new();

    // A leading comma is whisper marking the pause before the command — drop
    // it. A leading period belongs to the PREVIOUS sentence — keep it. The
    // trailing punctuation is part of the command utterance itself.
    let paragraph = PARAGRAPH.get_or_init(|| {
        regex::RegexBuilder::new(r",?\s*\b(new paragraph|nuovo paragrafo)\b[,.;]?\s*")
            .case_insensitive(true)
            .build()
            .expect("paragraph regex")
    });
    // This command SUPPLIES the period, so a leading one (whisper already
    // closed the sentence) is swallowed instead of doubling up.
    let full_stop = FULL_STOP.get_or_init(|| {
        regex::RegexBuilder::new(r"[,.]?\s*\b(punto e a capo|period new line)\b[,.;]?\s*")
            .case_insensitive(true)
            .build()
            .expect("full stop regex")
    });
    let bullet = BULLET.get_or_init(|| {
        regex::RegexBuilder::new(
            r",?\s*\b(nuovo punto elenco|punto elenco|elenco puntato|new bullet|bullet point)\b[,.;:]?\s*",
        )
        .case_insensitive(true)
        .build()
        .expect("bullet regex")
    });
    let line = LINE.get_or_init(|| {
        regex::RegexBuilder::new(r",?\s*\b(new line|a capo|nuova riga)\b[,.;]?\s*")
            .case_insensitive(true)
            .build()
            .expect("line regex")
    });

    // Longest phrases first: "punto e a capo" must not be half-eaten by
    // "a capo", nor "new paragraph" by "new line".
    let step1 = paragraph.replace_all(text, "\n\n");
    let step2 = full_stop.replace_all(&step1, ".\n");
    let step3 = bullet.replace_all(&step2, "\n- ");
    let step4 = line.replace_all(&step3, "\n");
    step4.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newline_commands_in_both_languages() {
        assert_eq!(apply_basic_commands("ciao a capo mondo"), "ciao\nmondo");
        assert_eq!(apply_basic_commands("ciao, nuova riga, mondo"), "ciao\nmondo");
        assert_eq!(apply_basic_commands("hello new line world"), "hello\nworld");
    }

    #[test]
    fn swallows_surrounding_punctuation() {
        assert_eq!(
            apply_basic_commands("First sentence. New line. Second one."),
            "First sentence.\nSecond one."
        );
    }

    #[test]
    fn paragraph_beats_line() {
        assert_eq!(
            apply_basic_commands("intro. Nuovo paragrafo. corpo"),
            "intro.\n\ncorpo"
        );
        assert_eq!(apply_basic_commands("a new paragraph b"), "a\n\nb");
    }

    #[test]
    fn punto_e_a_capo_supplies_the_period() {
        assert_eq!(
            apply_basic_commands("prima frase punto e a capo seconda frase"),
            "prima frase.\nseconda frase"
        );
        // Whisper already closed the sentence: no double period.
        assert_eq!(
            apply_basic_commands("Prima frase. Punto e a capo. Seconda frase."),
            "Prima frase.\nSeconda frase."
        );
        assert_eq!(
            apply_basic_commands("first period new line second"),
            "first.\nsecond"
        );
    }

    #[test]
    fn bullet_commands_in_both_languages() {
        assert_eq!(
            apply_basic_commands("la lista: punto elenco pane, punto elenco latte"),
            "la lista:\n- pane\n- latte"
        );
        assert_eq!(
            apply_basic_commands("shopping. New bullet bread. Bullet point milk."),
            "shopping.\n- bread.\n- milk."
        );
        assert_eq!(
            apply_basic_commands("temi, nuovo punto elenco primo"),
            "temi\n- primo"
        );
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(
            apply_basic_commands("una linea nuova di codice"),
            "una linea nuova di codice"
        );
        assert_eq!(apply_basic_commands("capolavoro a parte"), "capolavoro a parte");
    }
}
