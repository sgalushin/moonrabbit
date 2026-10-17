use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::catalog::{refresh_catalog, Catalog, WordList};
use crate::dictionary::word_json;
use crate::error::{ApiError, ApiResult};
use crate::records::{
    merge_examples, pos_sk, user_pk, validate_example, ExampleSentenceInput, ListMeta, Membership,
    QueueRecord, WordRecord,
};
use crate::store::Store;

pub fn list_summaries(catalog: &Catalog, sub: &str, studied: &HashSet<String>) -> Vec<Value> {
    let mut ids: Vec<&String> = catalog.lists.keys().collect();
    ids.sort();
    ids.into_iter()
        .filter(|id| catalog.lists[*id].owner == "system" || catalog.lists[*id].owner == sub)
        .filter_map(|id| summary(catalog, id, studied))
        .collect()
}

pub fn list_page(
    catalog: &Catalog,
    sub: &str,
    id: &str,
    studied: &HashSet<String>,
    cursor: usize,
) -> ApiResult<Value> {
    let list = catalog
        .lists
        .get(id)
        .filter(|list| list.owner == "system" || list.owner == sub)
        .ok_or_else(|| ApiError::missing("List not found."))?;
    if cursor > list.entries.len() {
        return Err(ApiError::bad("The list cursor is out of range."));
    }
    let end = cursor.saturating_add(100).min(list.entries.len());
    let words: Vec<Value> = list
        .entries
        .iter()
        .skip(cursor)
        .take(100)
        .filter_map(|(_, word_id)| word_json(catalog, word_id, studied.contains(word_id)))
        .collect();
    Ok(json!({
        "id": list.id,
        "title": list.title,
        "owner": list.owner,
        "words": words,
        "nextCursor": if end < list.entries.len() { Some(end) } else { None },
        "totalWords": list.entries.len(),
    }))
}
pub(crate) const MAX_WORD_ROWS: usize = 40;

fn validate_title(title: &str) -> ApiResult<&str> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 120 || title.len() > 480 {
        return Err(ApiError::bad(
            "Give the list a name of at most 120 characters.",
        ));
    }
    Ok(title)
}

pub async fn create_list(
    store: &Store,
    catalog: &mut Catalog,
    sub: &str,
    title: &str,
) -> ApiResult<Value> {
    let title = validate_title(title)?;
    refresh_catalog(store, catalog).await?;
    let id = format!("l{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let meta = ListMeta {
        pk: format!("LIST#{id}"),
        sk: "META".into(),
        title: title.into(),
        owner: sub.into(),
        deleted: false,
    };
    store
        .catalog_transaction(catalog.revision, vec![store.transaction_put(&meta)?])
        .await?;
    catalog.revision += 1;
    catalog.lists.insert(
        id.clone(),
        crate::catalog::WordList {
            id: id.clone(),
            title: title.into(),
            owner: sub.into(),
            entries: Vec::new(),
        },
    );
    Ok(json!({ "id": id, "title": title }))
}

pub async fn rename_list(
    store: &Store,
    catalog: &mut Catalog,
    sub: &str,
    id: &str,
    title: &str,
) -> ApiResult<()> {
    let title = validate_title(title)?;
    refresh_catalog(store, catalog).await?;
    owned(catalog, sub, id)?;
    let meta = ListMeta {
        pk: format!("LIST#{id}"),
        sk: "META".into(),
        title: title.into(),
        owner: sub.into(),
        deleted: false,
    };
    store
        .catalog_transaction(catalog.revision, vec![store.transaction_put(&meta)?])
        .await?;
    catalog.revision += 1;
    catalog.lists.get_mut(id).expect("owned list").title = title.into();
    Ok(())
}

pub async fn delete_list(
    store: &Store,
    catalog: &mut Catalog,
    sub: &str,
    id: &str,
) -> ApiResult<()> {
    refresh_catalog(store, catalog).await?;
    let list = owned(catalog, sub, id)?;
    // Tombstoning META makes arbitrarily large membership sets invisible in one
    // atomic commit; old membership rows cannot resurrect or overwrite a list.
    let meta = ListMeta {
        pk: format!("LIST#{id}"),
        sk: "META".into(),
        title: list.title.clone(),
        owner: sub.into(),
        deleted: true,
    };
    let mut writes = vec![store.transaction_put(&meta)?];
    let mut queue = QueueRecord {
        pk: user_pk(sub),
        sk: "QUEUE".into(),
        list_ids: catalog.queues.get(sub).cloned().unwrap_or_default(),
        version: catalog.queue_versions.get(sub).copied().unwrap_or(0),
    };
    let changed = queue.list_ids.iter().any(|item| item == id);
    let mut version = catalog.queue_versions.get(sub).copied().unwrap_or(0);
    if changed {
        queue.list_ids.retain(|item| item != id);
        version = version
            .checked_add(1)
            .ok_or_else(|| ApiError::unavailable("Queue version exhausted."))?;
        queue.version = version;
        writes.push(store.transaction_put(&queue)?);
    }
    store.catalog_transaction(catalog.revision, writes).await?;
    catalog.revision += 1;
    catalog.lists.remove(id);
    if changed {
        catalog.queues.insert(sub.into(), queue.list_ids);
        catalog.queue_versions.insert(sub.into(), version);
    }
    Ok(())
}

pub async fn set_queue(
    store: &Store,
    catalog: &mut Catalog,
    sub: &str,
    ids: Vec<String>,
    expected_version: u64,
) -> ApiResult<()> {
    refresh_catalog(store, catalog).await?;
    let version = catalog.queue_versions.get(sub).copied().unwrap_or(0);
    if version != expected_version {
        return Err(ApiError::conflict(
            "The queue changed in another window. Reload and try again.",
        ));
    }
    if ids.len() > 100
        || ids.iter().any(|id| {
            !catalog
                .lists
                .get(id)
                .is_some_and(|list| list.owner == "system" || list.owner == sub)
        })
    {
        return Err(ApiError::bad(
            "The queue contains an unavailable list or more than 100 lists.",
        ));
    }
    let mut unique = HashSet::new();
    if ids.iter().any(|id| !unique.insert(id)) {
        return Err(ApiError::bad(
            "Each list can appear in the queue only once.",
        ));
    }
    let version = version
        .checked_add(1)
        .ok_or_else(|| ApiError::unavailable("Queue version exhausted."))?;
    let record = QueueRecord {
        pk: user_pk(sub),
        sk: "QUEUE".into(),
        list_ids: ids,
        version,
    };
    store
        .catalog_transaction(catalog.revision, vec![store.transaction_put(&record)?])
        .await?;
    catalog.revision += 1;
    catalog.queues.insert(sub.into(), record.list_ids);
    catalog.queue_versions.insert(sub.into(), version);
    Ok(())
}

pub(crate) fn validated_word(word: &WordInput) -> ApiResult<WordRecord> {
    for (name, value, limit) in [
        ("Simplified", word.simplified.as_str(), 100),
        ("Pinyin", word.pinyin.as_str(), 300),
        ("Translation", word.translation.as_str(), 1000),
    ] {
        if value.trim().is_empty()
            || value.chars().count() > limit
            || value.len() > limit * 4
            || value.chars().any(char::is_control)
        {
            return Err(ApiError::bad(format!("{name} is required, must have at most {limit} characters, and cannot contain control characters.")));
        }
    }
    let mut record = WordRecord::new(&word.simplified, &word.pinyin, &word.translation);
    if let Some(spoken) = &word.pronounced_pinyin {
        if spoken.trim().is_empty()
            || spoken.chars().count() > 300
            || spoken.chars().any(char::is_control)
        {
            return Err(ApiError::bad(
                "Pronounced pinyin must have 1–300 characters and no control characters.",
            ));
        }
        record.pronounced_pinyin = Some(crate::pinyin::canonical_pinyin(spoken));
    }
    if word.examples.len() > 20 {
        return Err(ApiError::bad(
            "Import at most 20 source examples per word per request.",
        ));
    }
    let examples = word
        .examples
        .iter()
        .map(validate_example)
        .collect::<ApiResult<Vec<_>>>()?;
    merge_examples(&mut record.examples, &examples)?;
    if record.simplified.is_empty() || record.pinyin.is_empty() || record.translation.is_empty() {
        return Err(ApiError::bad(
            "Simplified, pinyin, and translation are required.",
        ));
    }
    Ok(record)
}

pub async fn add_words(
    store: &Store,
    catalog: &mut Catalog,
    sub: &str,
    list_id: &str,
    words: Vec<WordInput>,
) -> ApiResult<Value> {
    if words.len() > MAX_WORD_ROWS {
        return Err(ApiError::bad("Save at most 40 words per request."));
    }
    refresh_catalog(store, catalog).await?;
    let list = owned(catalog, sub, list_id)?;
    let mut next_pos = list.entries.last().map(|(pos, _)| *pos).unwrap_or(0);
    let have: HashSet<&str> = list.entries.iter().map(|(_, id)| id.as_str()).collect();
    let mut added_ids = HashSet::new();
    let mut rejected = Vec::new();
    let mut records: HashMap<String, WordRecord> = HashMap::new();
    let mut memberships = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let record = match validated_word(word) {
            Ok(record) => record,
            Err(error) => {
                rejected.push(json!({"index": index, "error": error.message()}));
                continue;
            }
        };
        let id = record.id().to_owned();
        if let Some(existing) = records.get(&id).or_else(|| catalog.words.get(&id)) {
            if !record.examples.is_empty() {
                let mut merged = existing.clone();
                match merge_examples(&mut merged.examples, &record.examples) {
                    Ok(true) => {
                        records.insert(id.clone(), merged);
                    }
                    Ok(false) => {}
                    Err(error) => {
                        rejected.push(json!({"index": index, "error": error.message()}));
                        continue;
                    }
                }
            }
        } else {
            records.insert(id.clone(), record);
        }
        // Source examples are merged even when this membership already exists.
        if !have.contains(id.as_str()) && added_ids.insert(id.clone()) {
            next_pos = next_pos
                .checked_add(1)
                .ok_or_else(|| ApiError::bad("This list is full."))?;
            memberships.push((next_pos, id));
        }
    }
    let mut writes = Vec::new();
    for record in records.values() {
        writes.push(store.transaction_put(record)?);
    }
    for (pos, id) in &memberships {
        writes.push(store.transaction_put(&Membership {
            pk: format!("LIST#{list_id}"),
            sk: pos_sk(*pos),
            word_id: id.clone(),
        })?);
    }
    let added = memberships.len();
    if !writes.is_empty() {
        store.catalog_transaction(catalog.revision, writes).await?;
        catalog.revision += 1;
        for (id, record) in records {
            catalog.words.insert(id, record);
        }
        catalog
            .lists
            .get_mut(list_id)
            .expect("owned list")
            .entries
            .extend(memberships);
    }
    Ok(json!({ "added": added, "rejected": rejected }))
}

pub async fn remove_word(
    store: &Store,
    catalog: &mut Catalog,
    sub: &str,
    list_id: &str,
    word_id: &str,
) -> ApiResult<()> {
    refresh_catalog(store, catalog).await?;
    let list = owned(catalog, sub, list_id)?;
    let Some(index) = list.entries.iter().position(|(_, id)| id == word_id) else {
        return Ok(());
    };
    let pos = list.entries[index].0;
    store
        .catalog_transaction(
            catalog.revision,
            vec![store.transaction_delete(&format!("LIST#{list_id}"), &pos_sk(pos))?],
        )
        .await?;
    catalog.revision += 1;
    catalog
        .lists
        .get_mut(list_id)
        .expect("owned list")
        .entries
        .remove(index);
    Ok(())
}
fn summary(catalog: &Catalog, id: &str, studied: &HashSet<String>) -> Option<Value> {
    let list = catalog.lists.get(id)?;
    let words_left = list
        .entries
        .iter()
        .filter(|(_, word)| !studied.contains(word))
        .count();
    let mut character_count = 0;
    let mut characters_studied = 0;
    for (_, id) in &list.entries {
        if catalog
            .words
            .get(id)
            .is_some_and(|word| word.simplified.chars().count() == 1)
        {
            character_count += 1;
            characters_studied += usize::from(studied.contains(id));
        }
    }
    Some(json!({
        "id": list.id,
        "title": list.title,
        "owner": list.owner,
        "words": list.entries.len(),
        "wordsLeft": words_left,
        "charactersStudied": characters_studied,
        "characterCount": character_count,
    }))
}

pub(crate) fn queue_rows(catalog: &Catalog, sub: &str, studied: &HashSet<String>) -> Vec<Value> {
    catalog
        .queues
        .get(sub)
        .into_iter()
        .flatten()
        .filter_map(|id| summary(catalog, id, studied))
        .collect()
}
fn owned<'a>(catalog: &'a Catalog, sub: &str, id: &str) -> ApiResult<&'a WordList> {
    let list = catalog
        .lists
        .get(id)
        .ok_or_else(|| ApiError::missing("List not found."))?;
    if list.owner != sub {
        return Err(ApiError::bad("That list cannot be edited."));
    }
    Ok(list)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordInput {
    pub simplified: String,
    pub pinyin: String,
    pub translation: String,
    #[serde(default)]
    pub pronounced_pinyin: Option<String>,
    #[serde(default)]
    pub examples: Vec<ExampleSentenceInput>,
}

#[cfg(test)]
mod tests {
    use super::*;
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
    #[test]
    fn pagination_covers_every_word_once_and_is_bounded() {
        let (catalog, ids) = catalog_with_words(205);
        let mut returned = Vec::new();
        let mut cursor = 0;
        loop {
            let page = list_page(&catalog, "u", "test", &HashSet::new(), cursor).unwrap();
            assert_eq!(page["totalWords"], 205);
            let words = page["words"].as_array().unwrap();
            assert!(words.len() <= 100);
            returned.extend(
                words
                    .iter()
                    .map(|word| word["id"].as_str().unwrap().to_string()),
            );
            match page["nextCursor"].as_u64() {
                Some(next) => {
                    assert!(next as usize > cursor);
                    cursor = next as usize;
                }
                None => break,
            }
        }
        assert_eq!(returned, ids);
        assert!(list_page(&catalog, "u", "test", &HashSet::new(), 206).is_err());
    }
    #[test]
    fn bounds_reject_invalid_rows_and_titles() {
        assert!(validate_title(&"x".repeat(121)).is_err());
        assert!(validated_word(&WordInput {
            simplified: "字".into(),
            pinyin: String::new(),
            translation: "character".into(),
            pronounced_pinyin: None,
            examples: Vec::new(),
        })
        .is_err());
    }

    #[test]
    fn manual_pronunciation_and_source_examples_are_kept_separately_from_identity() {
        let word = validated_word(&WordInput {
            simplified: "你好".into(),
            pinyin: "nǐ hǎo".into(),
            pronounced_pinyin: Some(" NÍ  HǍO ".into()),
            translation: "hello".into(),
            examples: vec![ExampleSentenceInput {
                simplified: "你好！".into(),
                pinyin: "nǐ hǎo!".into(),
                pronounced_pinyin: "ní hǎo!".into(),
                translation: "Hello!".into(),
            }],
        })
        .unwrap();
        assert_eq!(
            word.id(),
            WordRecord::new("你好", "nǐ hǎo", "different").id()
        );
        assert_eq!(word.pronounced_pinyin.as_deref(), Some("ní hǎo"));
        assert_eq!(word.examples.len(), 1);
        assert_eq!(word.examples[0].pronounced_pinyin, "ní hǎo!");
        let input: WordInput = serde_json::from_value(json!({
            "simplified":"你", "pinyin":"nǐ", "translation":"you"
        }))
        .unwrap();
        assert!(input.pronounced_pinyin.is_none());
        assert!(input.examples.is_empty());
    }
}
