use std::collections::{BTreeSet, HashMap};

use serde_json::{json, Value};

use crate::cards::Direction;
use crate::catalog::{refresh_catalog, Catalog};
use crate::config::Config;
use crate::enrichment::{self, EnrichedReading, EnrichedWord};
use crate::error::{ApiError, ApiResult};
use crate::lists::MAX_WORD_ROWS;
use crate::pinyin::{canonical_simplified, loose};
use crate::records::{
    card_sk, merge_examples, user_pk, validate_example, CardRecord, ExampleSentenceInput,
    WordRecord, MAX_EXAMPLES,
};
use crate::store::Store;

pub async fn word_detail(
    store: &Store,
    catalog: &Catalog,
    sub: &str,
    id: &str,
) -> ApiResult<Value> {
    let mut value =
        word_json(catalog, id, false).ok_or_else(|| ApiError::missing("Word not found."))?;
    value["examples"] = json!(catalog.words[id].examples);
    let recognition: Option<CardRecord> = store
        .get(&user_pk(sub), &card_sk(id, Direction::Recognition))
        .await?;
    let production: Option<CardRecord> = store
        .get(&user_pk(sub), &card_sk(id, Direction::Production))
        .await?;
    value["cards"] = json!([
        card_summary(recognition.as_ref(), Direction::Recognition),
        card_summary(production.as_ref(), Direction::Production),
    ]);
    Ok(value)
}

fn card_summary(card: Option<&CardRecord>, direction: Direction) -> Value {
    let Some(card) = card.filter(|card| card.schedule.reps > 0) else {
        return json!({ "direction": direction, "studied": false });
    };
    json!({
        "direction": direction,
        "studied": true,
        "state": card.schedule.state.key(),
        "reps": card.schedule.reps,
        "lapses": card.schedule.lapses,
        "due": card.schedule.due,
        "lastReview": card.schedule.last_review,
        "stability": card.schedule.stability,
    })
}

pub fn search(catalog: &Catalog, sub: &str, query: &str) -> Vec<Value> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let folded = loose(&needle);
    let mut matches: Vec<(bool, &WordRecord)> = catalog
        .words
        .values()
        .filter_map(|word| {
            let simplified = word.simplified.to_lowercase();
            let translation = word.translation.to_lowercase();
            let pinyin = loose(&word.pinyin);
            let exact = simplified == needle || translation == needle || pinyin == folded;
            (simplified.contains(&needle)
                || translation.contains(&needle)
                || pinyin.contains(&folded)
                || word
                    .pronounced_pinyin
                    .as_deref()
                    .is_some_and(|pinyin| loose(pinyin).contains(&folded)))
            .then_some((!exact, word))
        })
        .collect();
    matches.sort_by(|(a_rank, a), (b_rank, b)| {
        (a_rank, &a.simplified, &a.pinyin, &a.pk).cmp(&(b_rank, &b.simplified, &b.pinyin, &b.pk))
    });
    matches
        .into_iter()
        .take(30)
        .filter_map(|(_, word)| search_word_json(catalog, sub, word.id()))
        .collect()
}

fn search_word_json(catalog: &Catalog, sub: &str, id: &str) -> Option<Value> {
    let mut value = word_json(catalog, id, false)?;
    let mut lists: Vec<_> = catalog
        .lists
        .values()
        .filter(|list| {
            (list.owner == "system" || list.owner == sub)
                && list.entries.iter().any(|(_, word_id)| word_id == id)
        })
        .collect();
    lists.sort_by(|a, b| a.id.cmp(&b.id));
    value["lists"] = json!(lists
        .into_iter()
        .map(|list| json!({"id": list.id, "title": list.title}))
        .collect::<Vec<_>>());
    Some(value)
}

pub struct DraftData {
    lines: Vec<String>,
    generated: HashMap<String, Vec<EnrichedReading>>,
}

fn draft_lines(lines: Vec<String>) -> ApiResult<Vec<String>> {
    if lines.len() > MAX_WORD_ROWS {
        return Err(ApiError::bad("Enrich at most 40 lines per request."));
    }
    if lines.iter().any(|line| {
        line.len() > 400 || line.chars().count() > 100 || line.chars().any(char::is_control)
    }) {
        return Err(ApiError::bad(
            "Each draft line must have at most 100 characters and no control characters.",
        ));
    }
    Ok(lines
        .into_iter()
        .map(|line| canonical_simplified(&line))
        .collect())
}

// Provider calls must not hold the catalog mutex.
pub async fn fetch_drafts(
    http: &reqwest::Client,
    config: &Config,
    lines: Vec<String>,
) -> ApiResult<DraftData> {
    let lines = draft_lines(lines)?;
    let generated = if lines.iter().all(String::is_empty) {
        HashMap::new()
    } else {
        enrichment::fill_words(http, config, &lines)
            .await?
            .into_iter()
            .map(|word| (canonical_simplified(&word.simplified), word.readings))
            .collect()
    };
    Ok(DraftData { lines, generated })
}

fn normalized_reading(simplified: &str, reading: &EnrichedReading) -> ApiResult<WordRecord> {
    crate::lists::validated_word(&crate::lists::WordInput {
        simplified: simplified.to_owned(),
        pinyin: reading.pinyin.clone(),
        pronounced_pinyin: Some(reading.pronounced_pinyin.clone()),
        translation: reading.translation.clone(),
        examples: reading.examples.clone(),
    })
    .map_err(|error| ApiError::gateway(format!("Invalid dictionary response: {}", error.message())))
}

fn reading_json(record: &WordRecord) -> Value {
    let examples: Vec<_> = record
        .examples
        .iter()
        .map(|example| {
            json!({
                "simplified": example.simplified,
                "pinyin": example.pinyin,
                "pronouncedPinyin": example.pronounced_pinyin,
                "translation": example.translation,
            })
        })
        .collect();
    json!({
        "pinyin": record.pinyin,
        "pronouncedPinyin": record.pronounced_pinyin,
        "translation": record.translation,
        "examples": examples,
    })
}

pub fn assemble_drafts(data: DraftData) -> ApiResult<Vec<Value>> {
    let DraftData { lines, generated } = data;
    lines
        .into_iter()
        .map(|line| {
            let source = generated.get(&line);
            if !line.is_empty() && source.is_none_or(Vec::is_empty) {
                return Err(ApiError::gateway(
                    "The dictionary omitted a word. No incomplete result was returned.",
                ));
            }
            let readings = source
                .into_iter()
                .flatten()
                .map(|reading| {
                    normalized_reading(&line, reading).map(|record| reading_json(&record))
                })
                .collect::<ApiResult<Vec<_>>>()?;
            Ok(json!({ "simplified": line, "readings": readings }))
        })
        .collect()
}

pub fn screenshot_preview(
    catalog: &Catalog,
    sub: &str,
    words: Vec<EnrichedWord>,
) -> ApiResult<Value> {
    if words.len() > 200 {
        return Err(ApiError::gateway(
            "The screenshot exceeds 200 words. Crop a dense image and try again.",
        ));
    }
    let mut rows = Vec::new();
    for word in words {
        let simplified = canonical_simplified(&word.simplified);
        if word.readings.is_empty() || word.readings.len() > 12 {
            return Err(ApiError::gateway("The screenshot has an incomplete or overly dense reading list. Crop it and try again."));
        }
        let mut readings = Vec::with_capacity(word.readings.len());
        let mut matched = Vec::new();
        for reading in &word.readings {
            let record = normalized_reading(&simplified, reading)?;
            if let Some(existing) = catalog.words.get(record.id()) {
                matched.push(existing);
            }
            readings.push(reading_json(&record));
        }
        matched.sort_by(|a, b| (&a.pinyin, &a.pk).cmp(&(&b.pinyin, &b.pk)));
        let matches: Vec<_> = matched
            .into_iter()
            .filter_map(|existing| search_word_json(catalog, sub, existing.id()))
            .collect();
        rows.push(json!({"simplified": simplified, "readings": readings, "matches": matches}));
    }
    Ok(json!({"words": rows, "warnings": []}))
}

fn is_hanzi(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
}

fn hsk1_chars(catalog: &Catalog) -> ApiResult<BTreeSet<char>> {
    let list = catalog.lists.get("hsk-1").ok_or_else(|| {
        ApiError::unavailable("The HSK 1 list is missing. Restore it before generating examples.")
    })?;
    let chars: BTreeSet<_> = list
        .entries
        .iter()
        .filter_map(|(_, id)| catalog.words.get(id))
        .flat_map(|word| word.simplified.chars())
        .filter(|ch| is_hanzi(*ch))
        .collect();
    if chars.is_empty() {
        return Err(ApiError::unavailable(
            "The HSK 1 list has no characters. Restore it before generating examples.",
        ));
    }
    Ok(chars)
}

pub fn prepare_example(catalog: &Catalog, id: &str) -> ApiResult<(String, String)> {
    let word = catalog
        .words
        .get(id)
        .ok_or_else(|| ApiError::missing("Word not found."))?;
    let allowed = hsk1_chars(catalog)?;
    if word.examples.len() >= MAX_EXAMPLES {
        return Err(ApiError::unprocessable(
            "This word already has 100 examples. Existing examples were preserved.",
        ));
    }
    if !word.simplified.chars().any(is_hanzi)
        || word.simplified.chars().any(|ch| !allowed.contains(&ch))
    {
        return Err(ApiError::unprocessable(
            "This word contains characters outside the HSK 1 list. Choose an HSK 1 word to generate an HSK 1-only example.",
        ));
    }
    Ok((word.simplified.clone(), allowed.into_iter().collect()))
}

fn validate_generated_example(
    target: &str,
    allowed: &str,
    input: &ExampleSentenceInput,
) -> ApiResult<crate::records::ExampleSentence> {
    let example = validate_example(input).map_err(|error| {
        ApiError::unprocessable(format!(
            "The generated example is invalid: {} Try again.",
            error.message()
        ))
    })?;
    if !example.simplified.contains(target)
        || example.simplified.chars().any(|ch| {
            if is_hanzi(ch) {
                !allowed.contains(ch)
            } else {
                !ch.is_whitespace() && !crate::speech::sentence_punctuation(ch)
            }
        })
    {
        return Err(ApiError::unprocessable(
            "The generated sentence must contain the target word and only HSK 1 characters with ordinary punctuation. Try again.",
        ));
    }
    crate::speech::pronunciation_input(&example.pronounced_pinyin).map_err(|error| {
        ApiError::unprocessable(format!(
            "The generated pronunciation cannot be voiced: {} Try again.",
            error.message()
        ))
    })?;
    Ok(example)
}

pub async fn persist_example(
    store: &Store,
    catalog: &mut Catalog,
    id: &str,
    input: ExampleSentenceInput,
) -> ApiResult<()> {
    refresh_catalog(store, catalog).await?;
    let (target, allowed) = prepare_example(catalog, id)?;
    let example = validate_generated_example(&target, &allowed, &input)?;
    let mut word = catalog.words.get(id).expect("prepared word").clone();
    if !merge_examples(&mut word.examples, &[example])? {
        return Err(ApiError::unprocessable(
            "The provider repeated an existing example. Try again for a fresh sentence.",
        ));
    }
    store
        .catalog_transaction(catalog.revision, vec![store.transaction_put(&word)?])
        .await?;
    catalog.revision += 1;
    catalog.words.insert(id.to_owned(), word);
    Ok(())
}

pub(crate) fn word_json(catalog: &Catalog, word_id: &str, studied: bool) -> Option<Value> {
    let word = catalog.words.get(word_id)?;
    let mut value = json!({
        "id": word_id,
        "simplified": word.simplified,
        "pinyin": word.pinyin,
        "translation": word.translation,
        "studied": studied,
    });
    if let Some(spoken) = &word.pronounced_pinyin {
        value["pronouncedPinyin"] = json!(spoken);
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::WordList;

    fn catalog_with_word() -> (Catalog, String) {
        let mut catalog = Catalog::default();
        let word = WordRecord::new("你好", "nǐ hǎo", "hello");
        let id = word.id().to_owned();
        catalog.words.insert(id.clone(), word);
        for (list_id, owner) in [("hsk-1", "system"), ("mine", "u"), ("private", "other")] {
            catalog.lists.insert(
                list_id.into(),
                WordList {
                    id: list_id.into(),
                    title: list_id.into(),
                    owner: owner.into(),
                    entries: vec![(1, id.clone())],
                },
            );
        }
        (catalog, id)
    }

    fn example(text: &str) -> ExampleSentenceInput {
        ExampleSentenceInput {
            simplified: text.into(),
            pinyin: "nǐ hǎo!".into(),
            pronounced_pinyin: "ní hǎo!".into(),
            translation: "Hello!".into(),
        }
    }

    #[test]
    fn search_and_screenshot_expose_only_visible_memberships_in_stable_order() {
        let (catalog, id) = catalog_with_word();
        let expected = json!([{"id":"hsk-1","title":"hsk-1"},{"id":"mine","title":"mine"}]);
        let results = search(&catalog, "u", "你好");
        assert_eq!(results[0]["id"], id);
        assert_eq!(results[0]["lists"], expected);
        let preview = screenshot_preview(
            &catalog,
            "u",
            vec![EnrichedWord {
                simplified: "你好".into(),
                readings: vec![EnrichedReading {
                    pinyin: "nǐ hǎo".into(),
                    pronounced_pinyin: "ní hǎo".into(),
                    translation: "hello".into(),
                    examples: vec![example("你好！")],
                }],
            }],
        )
        .unwrap();
        assert_eq!(preview["words"][0]["matches"][0]["lists"], expected);
        assert!(
            catalog.words[&id].examples.is_empty(),
            "preview cannot mutate stored examples"
        );
        assert_eq!(
            preview["words"][0]["readings"][0]["examples"][0]["simplified"],
            "你好!"
        );
    }

    #[test]
    fn a_readable_screenshot_without_chinese_has_an_empty_preview() {
        let (catalog, _) = catalog_with_word();
        let preview = screenshot_preview(&catalog, "u", Vec::new()).unwrap();
        assert_eq!(preview["words"], json!([]));
    }

    #[test]
    fn missing_or_invalid_provider_readings_are_errors_not_partial_success() {
        assert!(assemble_drafts(DraftData {
            lines: vec!["字".into()],
            generated: HashMap::new()
        })
        .is_err());
        assert!(draft_lines(vec!["字".into(); 41]).is_err());
        let rows = assemble_drafts(DraftData {
            lines: vec![String::new()],
            generated: HashMap::new(),
        })
        .unwrap();
        assert_eq!(rows, vec![json!({"simplified":"","readings":[]})]);
    }

    #[test]
    fn generated_examples_enforce_target_hsk1_and_no_other_script() {
        let (mut catalog, id) = catalog_with_word();
        let (target, allowed) = prepare_example(&catalog, &id).unwrap();
        assert!(validate_generated_example(&target, &allowed, &example("你好！")).is_ok());
        for sentence in ["你很好！", "好！", "你好 hello", "你好123"] {
            assert!(
                validate_generated_example(&target, &allowed, &example(sentence)).is_err(),
                "{sentence}"
            );
        }
        let outside = WordRecord::new("猫", "māo", "cat");
        let outside_id = outside.id().to_owned();
        catalog.words.insert(outside_id.clone(), outside);
        assert!(prepare_example(&catalog, &outside_id).is_err());
    }

    #[test]
    fn search_keeps_exact_matches_first_and_is_independent_of_hash_iteration_order() {
        let mut catalog = Catalog::default();
        for index in 0..40 {
            let word = WordRecord::new(&format!("字{index:04}"), "zì", "character");
            catalog.words.insert(word.id().to_owned(), word);
        }
        let exact = WordRecord::new("字", "zì", "letter");
        let id = exact.id().to_owned();
        catalog.words.insert(id.clone(), exact);
        let expected = search(&catalog, "u", "字");
        assert_eq!(expected[0]["id"], id);
        assert_eq!(expected.len(), 30);
        let mut reversed: Vec<_> = catalog.words.drain().collect();
        reversed.sort_by(|a, b| b.0.cmp(&a.0));
        catalog.words.extend(reversed);
        assert_eq!(search(&catalog, "u", "字"), expected);
    }

    #[tokio::test]
    #[ignore = "requires LocalStack DynamoDB on http://127.0.0.1:4566"]
    async fn repeated_memberships_merge_examples_and_generated_examples_survive_reload() {
        use crate::lists::{add_words, WordInput};
        use aws_sdk_dynamodb::config::{Credentials, Region};
        use aws_sdk_dynamodb::types::{
            AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
        };

        let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(Region::new("us-east-2"))
            .credentials_provider(Credentials::new(
                "test",
                "test",
                None,
                None,
                "moonrabbit-test",
            ))
            .endpoint_url("http://127.0.0.1:4566")
            .load()
            .await;
        let client = aws_sdk_dynamodb::Client::new(&shared);
        let table = format!("moonrabbit-examples-{}", uuid::Uuid::new_v4().simple());
        client
            .create_table()
            .table_name(&table)
            .billing_mode(BillingMode::PayPerRequest)
            .attribute_definitions(
                AttributeDefinition::builder()
                    .attribute_name("pk")
                    .attribute_type(ScalarAttributeType::S)
                    .build()
                    .unwrap(),
            )
            .attribute_definitions(
                AttributeDefinition::builder()
                    .attribute_name("sk")
                    .attribute_type(ScalarAttributeType::S)
                    .build()
                    .unwrap(),
            )
            .key_schema(
                KeySchemaElement::builder()
                    .attribute_name("pk")
                    .key_type(KeyType::Hash)
                    .build()
                    .unwrap(),
            )
            .key_schema(
                KeySchemaElement::builder()
                    .attribute_name("sk")
                    .key_type(KeyType::Range)
                    .build()
                    .unwrap(),
            )
            .send()
            .await
            .unwrap();
        let store = Store::new(client.clone(), table.clone());
        let word = WordRecord::new("你好", "nǐ hǎo", "canonical hello");
        let id = word.id().to_owned();
        store.put(&word).await.unwrap();
        for (list_id, owner) in [("hsk-1", "system"), ("mine", "u")] {
            store
                .put(&crate::records::ListMeta {
                    pk: format!("LIST#{list_id}"),
                    sk: "META".into(),
                    title: list_id.into(),
                    owner: owner.into(),
                    deleted: false,
                })
                .await
                .unwrap();
            store
                .put(&crate::records::Membership {
                    pk: format!("LIST#{list_id}"),
                    sk: crate::records::pos_sk(1),
                    word_id: id.clone(),
                })
                .await
                .unwrap();
        }
        let mut catalog = Catalog::load(&store).await.unwrap();
        let imported = example("你好！");
        for _ in 0..2 {
            let result = add_words(
                &store,
                &mut catalog,
                "u",
                "mine",
                vec![WordInput {
                    simplified: "你好".into(),
                    pinyin: "nǐ hǎo".into(),
                    pronounced_pinyin: Some("wrong reading".into()),
                    translation: "must not overwrite".into(),
                    examples: vec![imported.clone()],
                }],
            )
            .await
            .unwrap();
            assert_eq!(result["added"], 0);
            assert_eq!(result["rejected"], json!([]));
        }
        let stored = &catalog.words[&id];
        assert_eq!(stored.translation, "canonical hello");
        assert_ne!(stored.pronounced_pinyin.as_deref(), Some("wrong reading"));
        assert_eq!(stored.examples.len(), 1);
        let old_id = stored.examples[0].id.clone();
        persist_example(&store, &mut catalog, &id, example("你好，你好！"))
            .await
            .unwrap();
        let mut reloaded = Catalog::load(&store).await.unwrap();
        let stored = &reloaded.words[&id];
        assert_eq!(stored.examples.len(), 2);
        assert_eq!(stored.examples[0].id, old_id);
        assert!(crate::speech::pronunciation_input(&stored.examples[1].pronounced_pinyin).is_ok());
        let detail = word_detail(&store, &reloaded, "u", &id).await.unwrap();
        assert_eq!(detail["examples"].as_array().unwrap().len(), 2);
        assert!(
            persist_example(&store, &mut reloaded, &id, example("你好，你好！"))
                .await
                .is_err()
        );
        assert_eq!(reloaded.words[&id].examples.len(), 2);

        let first = Catalog::load(&store).await.unwrap();
        let second = first.clone();
        let mut left_word = first.words[&id].clone();
        let mut right_word = second.words[&id].clone();
        merge_examples(
            &mut left_word.examples,
            &[validate_example(&example("你好？")).unwrap()],
        )
        .unwrap();
        merge_examples(
            &mut right_word.examples,
            &[validate_example(&example("你好。")).unwrap()],
        )
        .unwrap();
        store
            .catalog_transaction(
                first.revision,
                vec![store.transaction_put(&left_word).unwrap()],
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .catalog_transaction(
                    second.revision,
                    vec![store.transaction_put(&right_word).unwrap()]
                )
                .await,
            Err(ApiError::Conflict(_))
        ));
        let committed = Catalog::load(&store).await.unwrap();
        assert_eq!(committed.words[&id].examples.len(), 3);
        assert_eq!(committed.words[&id].examples[2].simplified, "你好?");
        client
            .delete_table()
            .table_name(table)
            .send()
            .await
            .unwrap();
    }
}
