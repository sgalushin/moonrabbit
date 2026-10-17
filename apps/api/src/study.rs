use std::collections::{HashMap, HashSet};

use chrono::Local;
use serde_json::{json, Value};

use crate::cards::{load_cards, studied_ids, CardPhase, CardState, Direction, Rating};
use crate::catalog::Catalog;
use crate::error::{ApiError, ApiResult};
use crate::records::{card_sk, user_pk, CardRecord, GradeRecord, WordRecord};
use crate::schedule::grade;
use crate::store::Store;

pub async fn session(
    store: &Store,
    catalog: &Catalog,
    sub: &str,
    unit: &str,
    amount: u32,
    direction: Direction,
) -> ApiResult<Value> {
    if !matches!(amount, 2 | 3 | 6 | 10 | 20) || !matches!(unit, "minutes" | "words") {
        return Err(ApiError::bad(
            "Choose Words or Minutes and 2, 3, 6, 10, or 20.",
        ));
    }
    let budget = if unit == "minutes" {
        amount * 8
    } else {
        amount
    };
    let cards = load_cards(store, sub).await?;
    let faces = session_faces(
        catalog,
        &cards,
        sub,
        budget as usize,
        direction,
        chrono::Utc::now().timestamp_millis(),
    );
    Ok(json!({ "cards": faces }))
}

const FUTURE_SKEW_MS: i64 = 5 * 60 * 1000;

fn session_faces(
    catalog: &Catalog,
    cards: &HashMap<String, CardRecord>,
    sub: &str,
    budget: usize,
    direction: Direction,
    now: i64,
) -> Vec<Value> {
    let eligible = |card: &CardRecord| {
        card.schedule.reps > 0
            && card.schedule.state != CardPhase::New
            && (card.schedule.due <= now || card.schedule.last_review > now + FUTURE_SKEW_MS)
    };
    let mut due: Vec<&CardRecord> = cards
        .values()
        .filter(|card| {
            eligible(card)
                && card.direction == direction
                && catalog.words.contains_key(&card.word_id)
        })
        .collect();
    due.sort_by(|a, b| {
        (a.schedule.due, &a.word_id, a.direction.key()).cmp(&(
            b.schedule.due,
            &b.word_id,
            b.direction.key(),
        ))
    });
    let mut chosen: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    {
        let mut select = |id| {
            if chosen.len() < budget && catalog.words.contains_key(id) && seen.insert(id) {
                chosen.push(id);
            }
            chosen.len() >= budget
        };
        let mut full = budget == 0;
        for card in due {
            full = select(card.word_id.as_str());
            if full {
                break;
            }
        }
        if !full {
            let mut backfill: Vec<&CardRecord> = cards
                .values()
                .filter(|card| {
                    card.direction != direction
                        && card.schedule.reps > 0
                        && cards
                            .get(&card_sk(&card.word_id, direction))
                            .is_none_or(|card| card.schedule.reps == 0)
                })
                .collect();
            backfill.sort_by(|a, b| {
                (a.schedule.last_review, &a.word_id).cmp(&(b.schedule.last_review, &b.word_id))
            });
            for card in backfill {
                if select(card.word_id.as_str()) {
                    break;
                }
            }
        }
    }
    'queue: for list_id in catalog.queues.get(sub).into_iter().flatten() {
        if chosen.len() >= budget {
            break;
        }
        let Some(list) = catalog
            .lists
            .get(list_id)
            .filter(|list| list.owner == "system" || list.owner == sub)
        else {
            continue;
        };
        for (_, id) in &list.entries {
            if chosen.len() >= budget {
                break 'queue;
            }
            let missing = cards
                .get(&card_sk(id, direction))
                .is_none_or(|card| card.schedule.reps == 0);
            if missing && catalog.words.contains_key(id) && seen.insert(id.as_str()) {
                chosen.push(id.as_str());
            }
        }
    }
    chosen
        .into_iter()
        .filter_map(|id| face(catalog, id, direction))
        .collect()
}

pub async fn apply_grade(
    store: &Store,
    sub: &str,
    event_id: &str,
    word_id: &str,
    direction: &str,
    rating: &str,
    reviewed_at: i64,
) -> ApiResult<Value> {
    let (direction, rating) = validate_grade(
        event_id,
        word_id,
        direction,
        rating,
        reviewed_at,
        chrono::Utc::now().timestamp_millis(),
    )?;
    if store
        .get::<WordRecord>(&format!("WORD#{word_id}"), "META")
        .await?
        .is_none()
    {
        return Err(ApiError::missing("Word not found."));
    }
    let pk = user_pk(sub);
    let grade_sk = format!("GRADE#{event_id}");
    let sk = card_sk(word_id, direction);
    let day_key = chrono::DateTime::from_timestamp_millis(reviewed_at)
        .ok_or_else(|| ApiError::bad("That review time is not valid."))?
        .with_timezone(&Local)
        .format("%Y-%m-%d")
        .to_string();
    for attempt in 0..16 {
        if let Some(event) = store.get::<GradeRecord>(&pk, &grade_sk).await? {
            if event.word_id != word_id
                || event.direction != direction.key()
                || event.rating != rating.key()
                || event.reviewed_at != reviewed_at
            {
                return Err(ApiError::bad(
                    "This event ID was already used for a different grade.",
                ));
            }
            return Ok(
                json!({ "status": if event.status == "stale" { "stale" } else { "duplicate" } }),
            );
        }
        let existing: Option<CardRecord> = store.get(&pk, &sk).await?;
        let mut current = existing
            .as_ref()
            .map(|card| card.schedule)
            .unwrap_or_else(CardState::fresh);
        let recovered_future_review = recover_future_timeline(
            &mut current,
            reviewed_at,
            chrono::Utc::now().timestamp_millis(),
        );
        let stale = current.last_review > 0 && reviewed_at < current.last_review;
        let event = GradeRecord {
            pk: pk.clone(),
            sk: grade_sk.clone(),
            word_id: word_id.into(),
            direction: direction.key().into(),
            rating: rating.key().into(),
            reviewed_at,
            status: if stale { "stale" } else { "applied" }.into(),
            recovered_future_review,
        };
        if stale {
            if store
                .grade_transaction(&event, None, existing.as_ref(), &format!("DAY#{day_key}"))
                .await?
            {
                return Ok(json!({ "status": "stale" }));
            }
        } else {
            let next = grade(&current, rating, reviewed_at)?;
            let card = CardRecord {
                pk: pk.clone(),
                sk: sk.clone(),
                word_id: word_id.into(),
                direction,
                schedule: next,
            };
            if store
                .grade_transaction(
                    &event,
                    Some((&card, existing.as_ref().map(|card| card.schedule.version))),
                    None,
                    &format!("DAY#{day_key}"),
                )
                .await?
            {
                return Ok(json!({ "status": "applied" }));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(2 * (attempt + 1))).await;
    }
    Err(ApiError::conflict(
        "This card changed repeatedly. Retry the same event.",
    ))
}

fn validate_grade(
    event_id: &str,
    word_id: &str,
    direction: &str,
    rating: &str,
    reviewed_at: i64,
    now: i64,
) -> ApiResult<(Direction, Rating)> {
    let direction =
        Direction::try_from(direction).map_err(|_| ApiError::bad("That grade is not valid."))?;
    let rating = Rating::try_from(rating).map_err(|_| ApiError::bad("That grade is not valid."))?;
    if event_id.is_empty()
        || event_id.len() > 128
        || !event_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || word_id.len() != 21
        || !word_id.starts_with('w')
        || !word_id.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
    {
        return Err(ApiError::bad("That grade is not valid."));
    }
    if reviewed_at <= 0
        || reviewed_at > now.saturating_add(FUTURE_SKEW_MS)
        || chrono::DateTime::from_timestamp_millis(reviewed_at).is_none()
    {
        return Err(ApiError::bad("Review time must be a valid historical time or within five minutes of the server clock."));
    }
    Ok((direction, rating))
}

fn recover_future_timeline(current: &mut CardState, reviewed_at: i64, now: i64) -> Option<i64> {
    if current.last_review <= now.saturating_add(FUTURE_SKEW_MS) {
        return None;
    }
    let poisoned = current.last_review;
    // Keep memory, repetitions, lapses and all historical events. Treat the
    // corrected answer as zero elapsed time rather than fabricating an interval.
    // The committed event records the replaced timestamp for audit/recovery.
    current.last_review = reviewed_at;
    current.due = reviewed_at;
    Some(poisoned)
}

pub async fn studied_set(store: &Store, sub: &str) -> ApiResult<HashSet<String>> {
    Ok(studied_ids(&load_cards(store, sub).await?))
}
fn face(catalog: &Catalog, word_id: &str, direction: Direction) -> Option<Value> {
    let word = catalog.words.get(word_id)?;
    let mut value = json!({
        "wordId": word_id,
        "direction": direction,
        "simplified": word.simplified,
        "pinyin": word.pinyin,
        "translation": word.translation,
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
    fn catalog_with_words(count: usize) -> (Catalog, Vec<String>) {
        let mut catalog = Catalog::default();
        let mut ids = Vec::new();
        for index in 0..count {
            let word = WordRecord::new(&format!("字{index:04}"), "zì", "character");
            ids.push(word.id().to_owned());
            catalog.words.insert(word.id().to_owned(), word);
        }
        catalog.lists.insert(
            "test".into(),
            WordList {
                id: "test".into(),
                title: "Test".into(),
                owner: "u".into(),
                entries: ids
                    .iter()
                    .enumerate()
                    .map(|(i, id)| ((i + 1) as u32, id.clone()))
                    .collect(),
            },
        );
        catalog.queues.insert("u".into(), vec!["test".into()]);
        (catalog, ids)
    }
    fn studied_card(id: &str, direction: Direction, due: i64) -> CardRecord {
        CardRecord {
            pk: user_pk("u"),
            sk: card_sk(id, direction),
            word_id: id.into(),
            direction,
            schedule: CardState {
                state: CardPhase::Review,
                step: 0,
                stability: 3.0,
                difficulty: 5.0,
                due,
                last_review: 1_000,
                reps: 3,
                lapses: 1,
                version: 3,
            },
        }
    }
    #[test]
    fn missing_direction_backfills_studied_words_outside_the_queue() {
        let (mut catalog, ids) = catalog_with_words(1);
        catalog.queues.clear();
        for (previous, selected) in [
            (Direction::Recognition, Direction::Production),
            (Direction::Production, Direction::Recognition),
        ] {
            let cards = HashMap::from([(
                card_sk(&ids[0], previous),
                studied_card(&ids[0], previous, 1_000_000),
            )]);
            let faces = session_faces(&catalog, &cards, "u", 2, selected, 10_000);
            assert_eq!(faces.len(), 1);
            assert_eq!(faces[0]["wordId"], ids[0]);
            assert_eq!(faces[0]["direction"], selected.key());
            assert!(session_faces(&catalog, &cards, "u", 2, previous, 10_000).is_empty());
        }
    }

    #[test]
    fn sessions_only_emit_the_selected_direction_and_prioritize_its_due_cards() {
        let (catalog, ids) = catalog_with_words(3);
        let cards = HashMap::from([
            (
                card_sk(&ids[2], Direction::Recognition),
                studied_card(&ids[2], Direction::Recognition, 2_000),
            ),
            (
                card_sk(&ids[2], Direction::Production),
                studied_card(&ids[2], Direction::Production, 1_000),
            ),
        ]);
        for direction in [Direction::Recognition, Direction::Production] {
            let faces = session_faces(&catalog, &cards, "u", 2, direction, 10_000);
            assert_eq!(faces.len(), 2);
            assert_eq!(faces[0]["wordId"], ids[2]);
            assert_eq!(faces[1]["wordId"], ids[0]);
            assert!(faces
                .iter()
                .all(|face| face["direction"] == direction.key()));
        }
    }

    #[test]
    fn an_opposite_due_card_cannot_pull_a_future_selected_card_into_session() {
        let (catalog, ids) = catalog_with_words(1);
        let cards = HashMap::from([
            (
                card_sk(&ids[0], Direction::Recognition),
                studied_card(&ids[0], Direction::Recognition, 1_000_000),
            ),
            (
                card_sk(&ids[0], Direction::Production),
                studied_card(&ids[0], Direction::Production, 2_000),
            ),
        ]);
        assert!(session_faces(&catalog, &cards, "u", 2, Direction::Recognition, 10_000).is_empty());
        let production = session_faces(&catalog, &cards, "u", 2, Direction::Production, 10_000);
        assert_eq!(production.len(), 1);
        assert_eq!(production[0]["direction"], "production");
    }
    #[test]
    fn bounds_reject_invalid_and_future_grades() {
        let (_, ids) = catalog_with_words(1);
        assert!(validate_grade("event", &ids[0], "recognition", "good", 1_000, 10_000).is_ok());
        assert!(validate_grade(
            "event",
            &ids[0],
            "recognition",
            "good",
            10_000 + FUTURE_SKEW_MS + 1,
            10_000
        )
        .is_err());
        assert!(validate_grade("", &ids[0], "recognition", "good", 1_000, 10_000).is_err());
        assert!(validate_grade(
            "event",
            "not-a-real-word",
            "recognition",
            "good",
            1_000,
            10_000
        )
        .is_err());
        for direction in ["", "Recognition", "retired"] {
            assert!(validate_grade("event", &ids[0], direction, "good", 1_000, 10_000).is_err());
        }
        for rating in ["", "hard", "easy", "GOOD"] {
            assert!(
                validate_grade("event", &ids[0], "recognition", rating, 1_000, 10_000).is_err()
            );
        }
    }
    #[test]
    fn future_recovery_preserves_memory_and_history_counters() {
        let mut current = studied_card("word", Direction::Recognition, 999_999_999).schedule;
        current.last_review = 999_999_000;
        assert_eq!(
            recover_future_timeline(&mut current, 8_000, 10_000),
            Some(999_999_000)
        );
        assert_eq!(current.last_review, 8_000);
        assert_eq!(current.due, 8_000);
        assert_eq!((current.reps, current.lapses, current.version), (3, 1, 3));
        assert_eq!((current.stability, current.difficulty), (3.0, 5.0));
        let next = grade(&current, Rating::Good, 8_000).unwrap();
        assert_eq!(next.reps, 4);
        assert_eq!(next.state, CardPhase::Review);
    }
}
#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::lists::{add_words, set_queue, WordInput};
    use crate::records::DayRecord;
    use aws_sdk_dynamodb::config::{Credentials, Region};
    use aws_sdk_dynamodb::types::{
        AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
    };

    // A dedicated table prevents this opt-in regression from touching user data.
    #[tokio::test]
    #[ignore = "requires LocalStack DynamoDB on http://127.0.0.1:4566"]
    async fn concurrent_grades_and_catalog_writers_preserve_every_committed_update() {
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
        let table = format!("moonrabbit-regression-{}", uuid::Uuid::new_v4().simple());
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
        let word = WordRecord::new("字", "zì", "character");
        store.put(&word).await.unwrap();
        let id = word.id().to_owned();
        let at = chrono::Utc::now().timestamp_millis();
        let mut requests = Vec::new();
        for index in 0..12 {
            let store = store.clone();
            let id = id.clone();
            requests.push(tokio::spawn(async move {
                apply_grade(
                    &store,
                    "regression",
                    &format!("event-{index}"),
                    &id,
                    "recognition",
                    "good",
                    at,
                )
                .await
            }));
        }
        for request in requests {
            assert_eq!(request.await.unwrap().unwrap()["status"], "applied");
        }
        let card: CardRecord = store
            .get(
                &user_pk("regression"),
                &card_sk(&id, Direction::Recognition),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(card.schedule.reps, 12);
        let day_sk = format!(
            "DAY#{}",
            chrono::DateTime::from_timestamp_millis(at)
                .unwrap()
                .with_timezone(&Local)
                .format("%Y-%m-%d")
        );
        let day: DayRecord = store
            .get(&user_pk("regression"), &day_sk)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((day.good, day.again), (12, 0));

        let mut requests = Vec::new();
        for _ in 0..12 {
            let store = store.clone();
            let id = id.clone();
            requests.push(tokio::spawn(async move {
                apply_grade(
                    &store,
                    "regression",
                    "duplicate-event",
                    &id,
                    "recognition",
                    "good",
                    at,
                )
                .await
            }));
        }
        let mut applied = 0;
        for request in requests {
            let status = request.await.unwrap().unwrap();
            if status["status"] == "applied" {
                applied += 1;
            } else {
                assert_eq!(status["status"], "duplicate");
            }
        }
        assert_eq!(applied, 1);
        let card: CardRecord = store
            .get(
                &user_pk("regression"),
                &card_sk(&id, Direction::Recognition),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(card.schedule.reps, 13);
        let event = GradeRecord {
            pk: user_pk("regression"),
            sk: "GRADE#must-not-commit".into(),
            word_id: id,
            direction: "recognition".into(),
            rating: "good".into(),
            reviewed_at: at,
            status: "applied".into(),
            recovered_future_review: None,
        };
        assert!(!store
            .grade_transaction(
                &event,
                Some((&card, Some(card.schedule.version - 1))),
                None,
                &day_sk
            )
            .await
            .unwrap());
        assert!(store
            .get::<GradeRecord>(&event.pk, &event.sk)
            .await
            .unwrap()
            .is_none());
        let day: DayRecord = store
            .get(&user_pk("regression"), &day_sk)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(day.good, 13);
        let id = &event.word_id;
        let missing_word = WordRecord::new("不存在", "bù cún zài", "missing");
        let missing_id = missing_word.id();
        assert!(matches!(
            apply_grade(
                &store,
                "regression",
                "missing-word",
                missing_id,
                "recognition",
                "good",
                at
            )
            .await,
            Err(ApiError::NotFound(_))
        ));
        assert!(store
            .get::<GradeRecord>(&event.pk, "GRADE#missing-word")
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            apply_grade(
                &store,
                "regression",
                "future",
                id,
                "recognition",
                "good",
                at + 365 * 86_400_000
            )
            .await,
            Err(ApiError::BadRequest(_))
        ));
        assert_eq!(
            apply_grade(
                &store,
                "regression",
                "older",
                id,
                "recognition",
                "good",
                at - 1
            )
            .await
            .unwrap()["status"],
            "stale"
        );
        assert_eq!(
            apply_grade(
                &store,
                "regression",
                "older",
                id,
                "recognition",
                "good",
                at - 1
            )
            .await
            .unwrap()["status"],
            "stale"
        );
        let day: DayRecord = store
            .get(&user_pk("regression"), &day_sk)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(day.good, 13);
        let mut poisoned = card;
        poisoned.schedule.last_review = at + 365 * 86_400_000;
        poisoned.schedule.due = poisoned.schedule.last_review + 86_400_000;
        store.put(&poisoned).await.unwrap();
        assert_eq!(
            apply_grade(
                &store,
                "regression",
                "recovered",
                id,
                "recognition",
                "good",
                at
            )
            .await
            .unwrap()["status"],
            "applied"
        );
        let recovered: CardRecord = store
            .get(&event.pk, &card_sk(id, Direction::Recognition))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.schedule.reps, 14);
        assert_eq!(recovered.schedule.last_review, at);
        let recovery: GradeRecord = store
            .get(&event.pk, "GRADE#recovered")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            recovery.recovered_future_review,
            Some(poisoned.schedule.last_review)
        );
        assert!(store
            .get::<GradeRecord>(&event.pk, "GRADE#event-0")
            .await
            .unwrap()
            .is_some());

        let mut bootstrap_left = Catalog::load(&store).await.unwrap();
        let mut bootstrap_right = bootstrap_left.clone();
        let (left, right) = tokio::join!(
            crate::catalog::ensure_user(&store, &mut bootstrap_left, "parallel-bootstrap"),
            crate::catalog::ensure_user(&store, &mut bootstrap_right, "parallel-bootstrap")
        );
        assert!(
            left.is_ok() && right.is_ok(),
            "Concurrent idempotent startup must complete in both processes"
        );
        let bootstrapped = Catalog::load(&store).await.unwrap();
        assert_eq!(
            bootstrapped.lists["my-parallel-bootstrap"].owner,
            "parallel-bootstrap"
        );
        assert_eq!(
            bootstrapped.queues["parallel-bootstrap"],
            ["my-parallel-bootstrap", "hsk-1"]
        );

        let mut first = Catalog::load(&store).await.unwrap();
        crate::catalog::ensure_user(&store, &mut first, "regression")
            .await
            .unwrap();
        let mut second = Catalog::load(&store).await.unwrap();
        let list_id = "my-regression";
        let input = |simplified: &str| {
            vec![WordInput {
                simplified: simplified.into(),
                pinyin: "zì".into(),
                translation: "character".into(),
                pronounced_pinyin: None,
                examples: Vec::new(),
            }]
        };
        let (left, right) = tokio::join!(
            add_words(&store, &mut first, "regression", list_id, input("甲")),
            add_words(&store, &mut second, "regression", list_id, input("乙"))
        );
        let snapshot = Catalog::load(&store).await.unwrap();
        for (result, simplified) in [(left, "甲"), (right, "乙")] {
            match result {
                Ok(_) => assert!(snapshot.lists[list_id]
                    .entries
                    .iter()
                    .any(|(_, id)| id == WordRecord::new(simplified, "zì", "character").id())),
                Err(error) => assert!(matches!(error, ApiError::Conflict(_))),
            }
        }
        let result = add_words(
            &store,
            &mut first,
            "regression",
            list_id,
            vec![
                WordInput {
                    simplified: "".into(),
                    pinyin: "zì".into(),
                    translation: "missing".into(),
                    pronounced_pinyin: None,
                    examples: Vec::new(),
                },
                WordInput {
                    simplified: "新".into(),
                    pinyin: "xīn".into(),
                    translation: "new".into(),
                    pronounced_pinyin: None,
                    examples: Vec::new(),
                },
                WordInput {
                    simplified: "坏".into(),
                    pinyin: "".into(),
                    translation: "bad".into(),
                    pronounced_pinyin: None,
                    examples: Vec::new(),
                },
                WordInput {
                    simplified: "新".into(),
                    pinyin: "xīn".into(),
                    translation: "new".into(),
                    pronounced_pinyin: None,
                    examples: Vec::new(),
                },
            ],
        )
        .await
        .unwrap();
        assert_eq!(result["added"], 1);
        assert_eq!(
            result["rejected"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["index"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [0, 2]
        );
        assert!(Catalog::load(&store).await.unwrap().lists[list_id]
            .entries
            .iter()
            .any(|(_, id)| id == WordRecord::new("新", "xīn", "new").id()));
        first = Catalog::load(&store).await.unwrap();
        second = first.clone();
        set_queue(&store, &mut first, "regression", vec![list_id.into()], 0)
            .await
            .unwrap();
        assert!(matches!(
            set_queue(&store, &mut second, "regression", vec![], 0).await,
            Err(ApiError::Conflict(_))
        ));
        let snapshot = Catalog::load(&store).await.unwrap();
        assert_eq!(snapshot.queues["regression"], [list_id]);
        client
            .delete_table()
            .table_name(table)
            .send()
            .await
            .unwrap();
    }
}
