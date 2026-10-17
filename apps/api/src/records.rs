use serde::{Deserialize, Serialize};

use crate::cards::{CardState, Direction};
use crate::pinyin::{canonical_pinyin, canonical_simplified, pronounced};

pub const MAX_EXAMPLES: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExampleSentenceInput {
    pub simplified: String,
    pub pinyin: String,
    pub pronounced_pinyin: String,
    pub translation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExampleSentence {
    pub id: String,
    pub simplified: String,
    pub pinyin: String,
    pub pronounced_pinyin: String,
    pub translation: String,
}

pub fn validate_example(input: &ExampleSentenceInput) -> crate::error::ApiResult<ExampleSentence> {
    use crate::error::ApiError;
    for (name, value, limit) in [
        ("Example sentence", input.simplified.as_str(), 500),
        ("Example pinyin", input.pinyin.as_str(), 1500),
        (
            "Example pronounced pinyin",
            input.pronounced_pinyin.as_str(),
            1500,
        ),
        ("Example translation", input.translation.as_str(), 2000),
    ] {
        if value.trim().is_empty()
            || value.chars().count() > limit
            || value.len() > limit * 4
            || value.chars().any(char::is_control)
        {
            return Err(ApiError::bad(format!(
                "{name} is required, must have at most {limit} characters, and cannot contain control characters."
            )));
        }
    }
    let simplified = canonical_simplified(&input.simplified);
    let pinyin = canonical_pinyin(&input.pinyin);
    Ok(ExampleSentence {
        id: format!("s{}", word_id(&simplified, &pinyin).trim_start_matches('w')),
        simplified,
        pinyin,
        pronounced_pinyin: canonical_pinyin(&input.pronounced_pinyin),
        translation: input.translation.trim().to_owned(),
    })
}

/// Preserve existing sentence IDs and text, including when an import repeats a sentence.
pub fn merge_examples(
    stored: &mut Vec<ExampleSentence>,
    incoming: &[ExampleSentence],
) -> crate::error::ApiResult<bool> {
    let mut additions: Vec<&ExampleSentence> = Vec::new();
    for example in incoming {
        if !stored.iter().chain(additions.iter().copied()).any(|old| {
            old.id == example.id
                || (old.simplified == example.simplified && old.pinyin == example.pinyin)
        }) {
            if stored.len() + additions.len() >= MAX_EXAMPLES {
                return Err(crate::error::ApiError::bad(
                    "This word already has 100 examples. No existing examples were removed.",
                ));
            }
            additions.push(example);
        }
    }
    let changed = !additions.is_empty();
    if changed {
        // Leave headroom below DynamoDB's 400 KiB item limit for canonical word fields.
        let bytes: usize = stored
            .iter()
            .chain(additions.iter().copied())
            .map(|example| {
                example.id.len()
                    + example.simplified.len()
                    + example.pinyin.len()
                    + example.pronounced_pinyin.len()
                    + example.translation.len()
                    + 128
            })
            .sum();
        if bytes > 300_000 {
            return Err(crate::error::ApiError::bad(
                "These examples exceed this word's storage limit. No existing examples were removed.",
            ));
        }
    }
    stored.extend(additions.into_iter().cloned());
    Ok(changed)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordRecord {
    pub pk: String,
    pub sk: String,
    pub simplified: String,
    pub pinyin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronounced_pinyin: Option<String>,
    pub translation: String,
    #[serde(default)]
    pub examples: Vec<ExampleSentence>,
}

impl WordRecord {
    pub fn new(simplified: &str, pinyin: &str, translation: &str) -> Self {
        let simplified = canonical_simplified(simplified);
        let pinyin = canonical_pinyin(pinyin);
        let pronounced_pinyin = pronounced(&simplified, &pinyin);
        Self {
            pk: format!("WORD#{}", word_id(&simplified, &pinyin)),
            sk: "META".into(),
            simplified,
            pinyin,
            pronounced_pinyin,
            translation: translation.trim().to_string(),
            examples: Vec::new(),
        }
    }

    pub fn id(&self) -> &str {
        self.pk.trim_start_matches("WORD#")
    }
}

fn word_id(simplified: &str, pinyin: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(simplified.as_bytes());
    hasher.update(b"\n");
    hasher.update(pinyin.as_bytes());
    format!("w{}", &hex::encode(hasher.finalize())[..20])
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMeta {
    pub pk: String,
    pub sk: String,
    pub title: String,
    pub owner: String,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    pub pk: String,
    pub sk: String,
    pub word_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueRecord {
    pub pk: String,
    pub sk: String,
    pub list_ids: Vec<String>,
    #[serde(default)]
    pub version: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardRecord {
    pub pk: String,
    pub sk: String,
    pub word_id: String,
    pub direction: Direction,
    #[serde(flatten)]
    pub schedule: CardState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradeRecord {
    pub pk: String,
    pub sk: String,
    pub word_id: String,
    pub direction: String,
    pub rating: String,
    pub reviewed_at: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_future_review: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayRecord {
    pub pk: String,
    pub sk: String,
    pub good: u32,
    pub again: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedRecord {
    pub pk: String,
    pub sk: String,
    pub version: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CatalogRevision {
    pub pk: String,
    pub sk: String,
    pub version: u64,
}

pub fn user_pk(sub: &str) -> String {
    format!("USER#{sub}")
}

pub fn card_sk(word_id: &str, direction: Direction) -> String {
    format!("CARD#{word_id}#{}", direction.key())
}

pub fn pos_sk(pos: u32) -> String {
    format!("POS#{pos:07}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{CardPhase, Rating};
    use serde_json::json;

    #[test]
    fn legacy_flat_cards_keep_all_schedule_fields_without_a_rewrite() {
        let legacy = json!({
            "pk": "USER#u", "sk": "CARD#w9dff1bf3d6d94dadbfe5#recognition",
            "wordId": "w9dff1bf3d6d94dadbfe5", "direction": "recognition",
            "state": "relearning", "step": 0, "stability": 3.5, "difficulty": 6.0,
            "due": 9_000, "lastReview": 8_000, "reps": 7, "lapses": 2, "version": 9,
            "pendingMs": 86_400_000,
        });
        let card: CardRecord = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(card.direction, Direction::Recognition);
        assert_eq!(card.schedule.state, CardPhase::Relearning);
        assert_eq!(
            (
                card.schedule.last_review,
                card.schedule.reps,
                card.schedule.lapses,
                card.schedule.version
            ),
            (8_000, 7, 2, 9)
        );
        let mut expected = legacy;
        expected.as_object_mut().unwrap().remove("pendingMs");
        assert_eq!(serde_json::to_value(&card).unwrap(), expected);
        let item = serde_dynamo::aws_sdk_dynamodb_1::to_item(&card).unwrap();
        let loaded: CardRecord = serde_dynamo::aws_sdk_dynamodb_1::from_item(item).unwrap();
        assert_eq!(serde_json::to_value(loaded).unwrap(), expected);
        let next = crate::schedule::grade(&card.schedule, Rating::Good, 9_000).unwrap();
        assert_eq!(next.reps, 8);
        assert_eq!(next.lapses, 2);
        assert_eq!(next.version, 10);
        assert_eq!(next.last_review, 9_000);
    }

    #[test]
    fn schedule_fields_remain_required_and_unknown_phases_are_rejected() {
        let state = serde_json::to_value(CardState::fresh()).unwrap();
        for field in [
            "state",
            "step",
            "stability",
            "difficulty",
            "due",
            "lastReview",
            "reps",
            "lapses",
            "version",
        ] {
            let mut missing = state.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<CardState>(missing).is_err(),
                "{field}"
            );
        }
        let mut unknown = state;
        unknown["state"] = json!("retired");
        assert!(serde_json::from_value::<CardState>(unknown).is_err());
    }

    #[test]
    fn historical_grades_without_outcome_metadata_still_deserialize() {
        let event: GradeRecord = serde_json::from_value(json!({
            "pk": "USER#u", "sk": "GRADE#old", "wordId": "w9dff1bf3d6d94dadbfe5",
            "direction": "recognition", "rating": "good", "reviewedAt": 8_000,
        }))
        .unwrap();
        assert_eq!(event.status, "");
        assert_eq!(event.recovered_future_review, None);
    }

    #[test]
    fn canonical_word_identity_keeps_historical_hash_bytes() {
        for (simplified, pinyin, expected, identity) in [
            (" 字 ", " ZÌ ", "w9dff1bf3d6d94dadbfe5", "字\nzì"),
            ("你好", "nǐ  hǎo", "w8b39efc3f83152279bd6", "你好\nnǐ hǎo"),
            ("三", "Ｓāｎ", "we851bc959aa787c9e6be", "三\nsān"),
        ] {
            let word = WordRecord::new(simplified, pinyin, "meaning");
            assert_eq!(word.id(), expected);
            assert_eq!(format!("{}\n{}", word.simplified, word.pinyin), identity);
            assert_eq!(
                WordRecord::new(&word.simplified, &word.pinyin, "other meaning").id(),
                expected
            );
        }
    }

    #[test]
    fn legacy_words_default_examples_and_keep_their_original_identity() {
        let word: WordRecord = serde_json::from_value(json!({
            "pk": "WORD#legacy-id", "sk": "META", "simplified": "你",
            "pinyin": "nǐ", "translation": "you"
        }))
        .unwrap();
        assert_eq!(word.id(), "legacy-id");
        assert!(word.examples.is_empty());
        assert_eq!(serde_json::to_value(&word).unwrap()["examples"], json!([]));
    }

    #[test]
    fn sentence_ids_are_canonical_and_merges_preserve_existing_text_atomically() {
        let input = ExampleSentenceInput {
            simplified: " 你好！ ".into(),
            pinyin: " NǏ  HǍO! ".into(),
            pronounced_pinyin: "ní hǎo!".into(),
            translation: "hello".into(),
        };
        let first = validate_example(&input).unwrap();
        let mut duplicate = first.clone();
        duplicate.translation = "changed translation".into();
        let mut stored = vec![first.clone()];
        assert!(!merge_examples(&mut stored, &[duplicate]).unwrap());
        assert_eq!(stored[0].translation, "hello");
        assert_eq!(stored[0].id, validate_example(&input).unwrap().id);
        stored = vec![first; MAX_EXAMPLES];
        let next = validate_example(&ExampleSentenceInput {
            simplified: "你好你好".into(),
            ..input
        })
        .unwrap();
        assert!(merge_examples(&mut stored, &[next]).is_err());
        assert_eq!(stored.len(), MAX_EXAMPLES);
    }
}
