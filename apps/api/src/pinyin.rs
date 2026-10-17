use unicode_normalization::UnicodeNormalization;

const TONE_ROWS: &[(char, [char; 4])] = &[
    ('a', ['ā', 'á', 'ǎ', 'à']),
    ('e', ['ē', 'é', 'ě', 'è']),
    ('i', ['ī', 'í', 'ǐ', 'ì']),
    ('o', ['ō', 'ó', 'ǒ', 'ò']),
    ('u', ['ū', 'ú', 'ǔ', 'ù']),
    ('ü', ['ǖ', 'ǘ', 'ǚ', 'ǜ']),
    ('n', ['ń', 'ń', 'ň', 'ǹ']),
];

struct Syl {
    chars: Vec<char>,
    mark_at: Option<usize>,
    tone: u8,
}

/// Dictionary pinyin used as the word identity: lower case, one space between syllables, tone marks kept.
pub fn canonical_pinyin(pinyin: &str) -> String {
    let normalized = pinyin.nfkc().collect::<String>().to_lowercase();
    let mut result = String::with_capacity(normalized.len());
    for syllable in normalized.split_whitespace() {
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(syllable);
    }
    result
}

pub fn canonical_simplified(text: &str) -> String {
    text.nfkc().collect::<String>().trim().to_string()
}

pub fn loose(text: &str) -> String {
    canonical_pinyin(text)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .map(|ch| match ch {
            'ā' | 'á' | 'ǎ' | 'à' => 'a',
            'ē' | 'é' | 'ě' | 'è' => 'e',
            'ī' | 'í' | 'ǐ' | 'ì' => 'i',
            'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
            'ū' | 'ú' | 'ǔ' | 'ù' => 'u',
            'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => 'ü',
            other => other,
        })
        .collect()
}

/// Spoken pinyin when sandhi changes a tone. `None` when it would copy `pinyin`.
pub fn pronounced(simplified: &str, pinyin: &str) -> Option<String> {
    let pinyin = canonical_pinyin(pinyin);
    let chars: Vec<char> = canonical_simplified(simplified).chars().collect();
    let mut syls: Vec<Syl> = pinyin.split_whitespace().map(parse_syllable).collect();
    if syls.len() != chars.len() || syls.is_empty() {
        return None;
    }
    let citation: Vec<u8> = syls.iter().map(|syl| syl.tone).collect();
    for index in 0..syls.len() {
        let next = citation.get(index + 1).copied().unwrap_or(0);
        match chars[index] {
            '一' if index == 0 || chars[index - 1] != '第' => {
                syls[index].tone = match next {
                    4 => 2,
                    1..=3 => 4,
                    _ => syls[index].tone,
                };
            }
            '不' if next == 4 => syls[index].tone = 2,
            _ => {}
        }
    }
    let mut index = 0;
    while index < syls.len() {
        if syls[index].tone != 3 {
            index += 1;
            continue;
        }
        let start = index;
        while index < syls.len() && syls[index].tone == 3 {
            index += 1;
        }
        if index - start >= 2 {
            for tone in &mut syls[start..index - 1] {
                tone.tone = 2;
            }
        }
    }
    let spoken = syls.iter().map(render).collect::<Vec<_>>().join(" ");
    (spoken != pinyin).then_some(spoken)
}

fn parse_syllable(text: &str) -> Syl {
    let chars: Vec<char> = text.chars().collect();
    let mut mark_at = None;
    let mut tone = 0;
    for (index, ch) in chars.iter().copied().enumerate() {
        if let Some(found) = tone_of(ch) {
            mark_at = Some(index);
            tone = found;
            break;
        }
    }
    Syl {
        chars,
        mark_at,
        tone,
    }
}

fn tone_of(ch: char) -> Option<u8> {
    for (_, marks) in TONE_ROWS {
        if let Some(index) = marks.iter().position(|mark| *mark == ch) {
            return Some((index + 1) as u8);
        }
    }
    None
}

fn bare(ch: char) -> char {
    for (base, marks) in TONE_ROWS {
        if marks.contains(&ch) {
            return *base;
        }
    }
    ch
}

fn render(syl: &Syl) -> String {
    syl.chars
        .iter()
        .copied()
        .enumerate()
        .map(|(index, ch)| {
            if syl.mark_at != Some(index) {
                return ch;
            }
            let base = bare(ch);
            if syl.tone == 0 {
                base
            } else {
                marked(base, syl.tone).unwrap_or(base)
            }
        })
        .collect()
}

fn marked(base: char, tone: u8) -> Option<char> {
    let row = TONE_ROWS.iter().find(|(letter, _)| *letter == base)?;
    row.1.get((tone as usize).wrapping_sub(1)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_changes_the_first_third_tone() {
        assert_eq!(pronounced("你好", "nǐ hǎo").as_deref(), Some("ní hǎo"));
    }

    #[test]
    fn unchanged_reading_is_omitted() {
        assert_eq!(pronounced("杯子", "bēi zi"), None);
    }

    #[test]
    fn yi_and_bu_follow_the_next_tone() {
        assert_eq!(pronounced("一下", "yī xià").as_deref(), Some("yí xià"));
        assert_eq!(pronounced("一些", "yī xiē").as_deref(), Some("yì xiē"));
        assert_eq!(pronounced("不是", "bù shì").as_deref(), Some("bú shì"));
        assert_eq!(pronounced("不好", "bù hǎo"), None);
    }

    #[test]
    fn ordinal_yi_stays_first_tone() {
        assert_eq!(pronounced("第一", "dì yī"), None);
        assert_eq!(pronounced("第一手", "dì yī shǒu"), None);
    }

    #[test]
    fn case_and_spacing_share_an_identity() {
        assert_eq!(canonical_pinyin("Sān"), canonical_pinyin("sān"));
        assert_eq!(canonical_pinyin("nǐ  hǎo"), "nǐ hǎo");
    }
}
