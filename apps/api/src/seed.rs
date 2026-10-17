use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::{storage, ApiResult};
use crate::records::{pos_sk, ListMeta, Membership, SeedRecord, WordRecord};
use crate::store::Store;

const VERSION: u32 = 1;

#[derive(Deserialize)]
struct Dump {
    title: String,
    words: Vec<DumpWord>,
}

#[derive(Deserialize)]
struct DumpWord {
    simplified: String,
    pinyin: String,
    #[serde(default)]
    english: String,
    #[serde(default)]
    definitions: Vec<String>,
}

pub async fn ensure(store: &Store) -> ApiResult<()> {
    let existing: Option<SeedRecord> = store.get("SEED", "HSK").await?;
    if existing.is_some_and(|seed| seed.version == VERSION) {
        tracing::info!("hsk catalog already seeded");
        return Ok(());
    }
    let dir = data_dir()?;
    let files = [
        "hsk-3-0-level-1.json",
        "hsk-3-0-level-2.json",
        "hsk-3-0-level-3.json",
        "hsk-3-0-level-4.json",
        "hsk-3-0-level-5.json",
        "hsk-3-0-level-6.json",
        "hsk-3-0-levels-7-9.json",
    ];
    let mut words: Vec<WordRecord> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut lists: Vec<(ListMeta, Vec<Membership>)> = Vec::new();
    for file in files {
        let dump: Dump = serde_json::from_str(
            &fs::read_to_string(dir.join(file))
                .map_err(|error| storage(error, "The HSK seed could not be read."))?,
        )
        .map_err(|error| storage(error, "The HSK seed could not be read."))?;
        let list_id = list_id_for(file);
        let meta = ListMeta {
            pk: format!("LIST#{list_id}"),
            sk: "META".into(),
            title: dump.title,
            owner: "system".into(),
            deleted: false,
        };
        let mut membership = Vec::new();
        let mut placed = std::collections::HashSet::new();
        for (index, word) in dump.words.into_iter().enumerate() {
            let record = WordRecord::new(
                &word.simplified,
                &word.pinyin,
                &gloss(&word.definitions, &word.english),
            );
            let id = record.id().to_owned();
            if seen.insert(record.pk.clone()) {
                words.push(record);
            }
            if placed.insert(id.clone()) {
                membership.push(Membership {
                    pk: meta.pk.clone(),
                    sk: pos_sk((index + 1) as u32),
                    word_id: id,
                });
            }
        }
        lists.push((meta, membership));
    }
    tracing::info!(words = words.len(), "writing hsk words");
    store.batch_put(&words).await?;
    for (meta, membership) in &lists {
        store.put(meta).await?;
        tracing::info!(list = %meta.title, rows = membership.len(), "writing list");
        store.batch_put(membership).await?;
    }
    store
        .put(&SeedRecord {
            pk: "SEED".into(),
            sk: "HSK".into(),
            version: VERSION,
        })
        .await?;
    Ok(())
}

fn list_id_for(file: &str) -> String {
    if file.contains("7-9") {
        "hsk-7-9".into()
    } else {
        file.trim_start_matches("hsk-3-0-")
            .trim_end_matches(".json")
            .replace("level-", "hsk-")
    }
}

fn data_dir() -> ApiResult<PathBuf> {
    for candidate in [
        "data/lists/hsk-3.0-2026",
        "../../data/lists/hsk-3.0-2026",
        "../data/lists/hsk-3.0-2026",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_dir() {
            return Ok(path);
        }
    }
    Err(storage(
        "missing seed directory",
        "The HSK seed could not be read.",
    ))
}

fn gloss(definitions: &[String], english: &str) -> String {
    let mut senses: Vec<&str> = definitions
        .iter()
        .map(|sense| sense.trim())
        .filter(|sense| !sense.is_empty() && useful(sense))
        .collect();
    if senses.is_empty() {
        senses = english
            .split('/')
            .map(str::trim)
            .filter(|sense| !sense.is_empty() && useful(sense))
            .collect();
    }
    if senses.len() > 1 && senses[0].to_lowercase().starts_with("surname") {
        senses.remove(0);
    }
    let mut text = senses.first().copied().unwrap_or("—").to_string();
    if let Some(rest) = text.strip_prefix("PN:") {
        text = rest.trim().to_string();
    }
    if let Some((head, _)) = text.split_once(',') {
        if head.trim().chars().count() >= 2 {
            text = head.trim().to_string();
        }
    }
    if text.chars().count() > 80 {
        let end = text
            .char_indices()
            .nth(80)
            .map(|(index, _)| index)
            .unwrap_or(text.len());
        let slice = &text[..end];
        text = slice
            .rfind([',', ';', '/'])
            .map(|index| slice[..index].trim().to_string())
            .filter(|cut| !cut.is_empty())
            .unwrap_or_else(|| slice.trim().to_string());
    }
    if text.is_empty() {
        "—".into()
    } else {
        text
    }
}

fn useful(sense: &str) -> bool {
    let lower = sense.to_lowercase();
    !lower.starts_with("also pr")
        && !lower.contains("radical")
        && !lower.starts_with("cl:")
        && !lower.starts_with("classifier")
        && !sense.contains("[yao")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surname_is_not_the_learner_gloss() {
        let text = gloss(
            &["surname San".into(), "three".into(), "3".into()],
            "surname San/three/3",
        );
        assert_eq!(text, "three");
    }

    #[test]
    fn proper_name_falls_back_to_a_short_label() {
        assert_eq!(gloss(&[], "PN:China, People's Republic"), "China");
    }
}
