use std::error::Error;
use std::fmt;

use crate::KeySequence;

/// Result of encoding space-separated, tone-free pinyin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedPinyin {
    /// Concatenation of all full two-key syllable codes.
    pub full_code: KeySequence,
    /// One two-key code per pinyin syllable.
    pub syllable_codes: Vec<KeySequence>,
}

/// Encodes a space-separated pinyin phrase with the Ziranma mapping.
///
/// The codec accepts ASCII pinyin, `v`, `ü`, and `u:` for ü. It applies the
/// mapping table but does not attempt to prove that every initial/final
/// combination is a legal Mandarin syllable.
pub fn encode_pinyin_phrase(pinyin: &str) -> Result<EncodedPinyin, PinyinEncodeError> {
    let syllables = pinyin.split_whitespace().collect::<Vec<_>>();
    if syllables.is_empty() {
        return Err(PinyinEncodeError::EmptyPhrase);
    }

    let mut full_code = String::with_capacity(syllables.len() * 2);
    let mut syllable_codes = Vec::with_capacity(syllables.len());
    for syllable in syllables {
        let code = encode_pinyin_syllable(syllable)?;
        full_code.push_str(code.as_str());
        syllable_codes.push(code);
    }

    Ok(EncodedPinyin {
        full_code: KeySequence::new(full_code).expect("the codec only emits lowercase ASCII"),
        syllable_codes,
    })
}

/// Encodes one tone-free pinyin syllable as two Ziranma keys.
pub fn encode_pinyin_syllable(syllable: &str) -> Result<KeySequence, PinyinEncodeError> {
    let normalized = normalize_syllable(syllable)?;

    if normalized.starts_with(['a', 'e', 'o']) {
        let code = match normalized.as_str() {
            "a" => "aa",
            "o" => "oo",
            "e" => "ee",
            "ai" => "al",
            "ei" => "ez",
            "ao" => "ak",
            "ou" => "ob",
            "an" => "aj",
            "en" => "ef",
            "ang" => "ah",
            "eng" => "eg",
            "ong" => "os",
            "er" => "er",
            _ => {
                return Err(PinyinEncodeError::UnsupportedSyllable {
                    syllable: syllable.to_owned(),
                });
            }
        };
        return Ok(KeySequence::new(code).expect("the mapping table is lowercase ASCII"));
    }

    let (initial, mut final_part) =
        split_initial(&normalized).ok_or_else(|| PinyinEncodeError::UnsupportedSyllable {
            syllable: syllable.to_owned(),
        })?;

    // In the Ziranma layout, bare ü after j/q/x/y is represented by v.
    if matches!(initial, "j" | "q" | "x" | "y") && final_part == "u" {
        final_part = "v";
    }

    let initial_key = match initial {
        "zh" => 'v',
        "ch" => 'i',
        "sh" => 'u',
        single => single
            .chars()
            .next()
            .expect("a recognized initial is never empty"),
    };
    let final_key = match final_part {
        "iu" => 'q',
        "ia" | "ua" => 'w',
        "uan" | "van" => 'r',
        "ue" | "ve" => 't',
        "ing" | "uai" => 'y',
        "uo" => 'o',
        "un" | "vn" => 'p',
        "ong" | "iong" => 's',
        "iang" | "uang" => 'd',
        "en" => 'f',
        "eng" => 'g',
        "ang" => 'h',
        "ian" => 'm',
        "an" => 'j',
        "iao" => 'c',
        "ao" => 'k',
        "ai" => 'l',
        "ei" => 'z',
        "ie" => 'x',
        "ui" => 'v',
        "ou" => 'b',
        "in" => 'n',
        "a" => 'a',
        "e" => 'e',
        "i" => 'i',
        "o" => 'o',
        "u" => 'u',
        "v" => 'v',
        _ => {
            return Err(PinyinEncodeError::UnsupportedSyllable {
                syllable: syllable.to_owned(),
            });
        }
    };

    Ok(KeySequence::new(format!("{initial_key}{final_key}"))
        .expect("the mapping table is lowercase ASCII"))
}

fn normalize_syllable(syllable: &str) -> Result<String, PinyinEncodeError> {
    let normalized = syllable.to_lowercase().replace("u:", "v").replace('ü', "v");
    if normalized.is_empty()
        || !normalized
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_lowercase())
    {
        return Err(PinyinEncodeError::InvalidCharacters {
            syllable: syllable.to_owned(),
        });
    }
    Ok(normalized)
}

fn split_initial(syllable: &str) -> Option<(&str, &str)> {
    const INITIALS: [&str; 23] = [
        "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x",
        "r", "z", "c", "s", "y", "w",
    ];

    INITIALS.iter().find_map(|initial| {
        syllable
            .strip_prefix(initial)
            .filter(|final_part| !final_part.is_empty())
            .map(|final_part| (*initial, final_part))
    })
}

/// Error returned when pinyin cannot be mapped by the baseline codec.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PinyinEncodeError {
    /// The phrase contained no syllables.
    EmptyPhrase,
    /// A syllable contained tones, punctuation, or other unsupported text.
    InvalidCharacters {
        /// Original source syllable.
        syllable: String,
    },
    /// The syllable shape is not covered by the Ziranma mapping table.
    UnsupportedSyllable {
        /// Original source syllable.
        syllable: String,
    },
}

impl fmt::Display for PinyinEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPhrase => write!(formatter, "拼音短语不能为空"),
            Self::InvalidCharacters { syllable } => write!(
                formatter,
                "拼音音节 {syllable:?} 含有不支持的字符；请使用无声调拼音"
            ),
            Self::UnsupportedSyllable { syllable } => {
                write!(formatter, "暂不支持拼音音节 {syllable:?}")
            }
        }
    }
}

impl Error for PinyinEncodeError {}

#[cfg(test)]
mod tests {
    use super::{encode_pinyin_phrase, encode_pinyin_syllable};

    #[test]
    fn encodes_every_mapped_final_family() {
        let cases = [
            ("jiu", "jq"),
            ("xia", "xw"),
            ("gua", "gw"),
            ("juan", "jr"),
            ("jue", "jt"),
            ("ying", "yy"),
            ("shuang", "ud"),
            ("guo", "go"),
            ("lun", "lp"),
            ("xiong", "xs"),
            ("liang", "ld"),
            ("ben", "bf"),
            ("feng", "fg"),
            ("bang", "bh"),
            ("mian", "mm"),
            ("lan", "lj"),
            ("biao", "bc"),
            ("hao", "hk"),
            ("lai", "ll"),
            ("fei", "fz"),
            ("jie", "jx"),
            ("gui", "gv"),
            ("dou", "db"),
            ("lin", "ln"),
        ];

        for (pinyin, expected) in cases {
            assert_eq!(
                encode_pinyin_syllable(pinyin).unwrap().as_str(),
                expected,
                "{pinyin}"
            );
        }
    }

    #[test]
    fn encodes_zero_initial_syllables() {
        let cases = [
            ("a", "aa"),
            ("o", "oo"),
            ("e", "ee"),
            ("ai", "al"),
            ("ei", "ez"),
            ("ao", "ak"),
            ("ou", "ob"),
            ("an", "aj"),
            ("en", "ef"),
            ("ang", "ah"),
            ("eng", "eg"),
            ("ong", "os"),
            ("er", "er"),
        ];

        for (pinyin, expected) in cases {
            assert_eq!(
                encode_pinyin_syllable(pinyin).unwrap().as_str(),
                expected,
                "{pinyin}"
            );
        }
    }

    #[test]
    fn handles_compound_initials_and_umlaut_aliases() {
        let cases = [
            ("zhi", "vi"),
            ("chi", "ii"),
            ("shi", "ui"),
            ("ju", "jv"),
            ("nü", "nv"),
            ("lu:", "lv"),
            ("lüe", "lt"),
        ];

        for (pinyin, expected) in cases {
            assert_eq!(
                encode_pinyin_syllable(pinyin).unwrap().as_str(),
                expected,
                "{pinyin}"
            );
        }
    }

    #[test]
    fn encodes_phrase_and_preserves_syllable_boundaries() {
        let encoded = encode_pinyin_phrase("ni hao").unwrap();
        assert_eq!(encoded.full_code.as_str(), "nihk");
        assert_eq!(
            encoded
                .syllable_codes
                .iter()
                .map(|code| code.as_str())
                .collect::<Vec<_>>(),
            ["ni", "hk"]
        );
    }

    #[test]
    fn rejects_tones_and_unsupported_shapes() {
        assert!(encode_pinyin_syllable("nǐ").is_err());
        assert!(encode_pinyin_syllable("xyz").is_err());
        assert!(encode_pinyin_phrase("   ").is_err());
    }
}
