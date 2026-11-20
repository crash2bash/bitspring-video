//! Telling whether two language codes mean the same language.
//!
//! Files tag tracks with ISO 639 codes, but not always the same edition: "en", "eng"; "de",
//! "deu", and the older "ger". File names add their own habits ("film.ru.srt").

/// (two-letter, three-letter, older three-letter) for the languages films usually come in.
const CODES: &[(&str, &str, &str)] = &[
    ("en", "eng", "eng"),
    ("ru", "rus", "rus"),
    ("uk", "ukr", "ukr"),
    ("ka", "kat", "geo"),
    ("de", "deu", "ger"),
    ("fr", "fra", "fre"),
    ("es", "spa", "spa"),
    ("it", "ita", "ita"),
    ("pt", "por", "por"),
    ("pl", "pol", "pol"),
    ("nl", "nld", "dut"),
    ("sv", "swe", "swe"),
    ("no", "nor", "nor"),
    ("da", "dan", "dan"),
    ("fi", "fin", "fin"),
    ("cs", "ces", "cze"),
    ("el", "ell", "gre"),
    ("tr", "tur", "tur"),
    ("he", "heb", "heb"),
    ("ar", "ara", "ara"),
    ("hi", "hin", "hin"),
    ("ja", "jpn", "jpn"),
    ("ko", "kor", "kor"),
    ("zh", "zho", "chi"),
];

/// One spelling for every code of a language; unknown codes stand for themselves.
fn canonical(code: &str) -> String {
    let code = code.trim().to_ascii_lowercase();
    // "pt-BR", "zh_Hans": the language is the part before the region or script.
    let base = code.split(['-', '_']).next().unwrap_or("").to_string();
    CODES
        .iter()
        .find(|(two, three, old)| base == *two || base == *three || base == *old)
        .map(|(_, three, _)| three.to_string())
        .unwrap_or(base)
}

/// Whether two codes name the same language. An empty or undetermined code matches nothing.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (canonical(a), canonical(b));
    !a.is_empty() && a != "und" && a == b
}

#[cfg(test)]
mod tests {
    use super::same;

    #[test]
    fn editions_of_the_same_code_agree() {
        assert!(same("en", "eng"));
        assert!(same("ger", "deu"));
        assert!(same("de", "ger"));
        assert!(same("RU", "rus"));
        assert!(same("geo", "ka"));
    }

    #[test]
    fn regions_do_not_split_a_language() {
        assert!(same("pt-BR", "por"));
        assert!(same("zh_Hans", "chi"));
    }

    #[test]
    fn different_or_missing_languages_do_not_match() {
        assert!(!same("eng", "rus"));
        assert!(!same("", ""));
        assert!(!same("und", "und"));
        assert!(!same("eng", ""));
    }

    #[test]
    fn unknown_codes_match_only_themselves() {
        assert!(same("tlh", "tlh"));
        assert!(!same("tlh", "eng"));
    }
}
