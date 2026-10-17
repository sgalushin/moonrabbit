use std::collections::HashMap;

use serde_dynamo::aws_sdk_dynamodb_1::from_item;

use crate::error::{ApiError, ApiResult};
use crate::records::{ListMeta, Membership, QueueRecord, WordRecord};
use crate::store::{string_attr, Store};

#[derive(Clone)]
pub struct WordList {
    pub id: String,
    pub title: String,
    pub owner: String,
    pub entries: Vec<(u32, String)>,
}

#[derive(Clone, Default)]
pub struct Catalog {
    pub words: HashMap<String, WordRecord>,
    pub lists: HashMap<String, WordList>,
    pub queues: HashMap<String, Vec<String>>,
    pub revision: u64,
    pub queue_versions: HashMap<String, u64>,
}

impl Catalog {
    pub async fn load(store: &Store) -> ApiResult<Self> {
        for _ in 0..6 {
            let revision = store.catalog_version().await?;
            let catalog = Self::load_snapshot(store, revision).await?;
            if store.catalog_version().await? == revision {
                return Ok(catalog);
            }
        }
        Err(ApiError::conflict("The catalog is busy. Try again."))
    }

    async fn load_snapshot(store: &Store, revision: u64) -> ApiResult<Self> {
        let items = store.scan().await?;
        let mut catalog = Catalog {
            revision,
            ..Catalog::default()
        };
        let mut membership = Vec::new();
        for item in items {
            let pk = string_attr(&item, "pk");
            let sk = string_attr(&item, "sk");
            if pk.starts_with("WORD#") && sk == "META" {
                if let Ok(word) = from_item::<WordRecord>(item) {
                    catalog.words.insert(word.id().to_owned(), word);
                }
            } else if pk.starts_with("LIST#") && sk == "META" {
                if let Ok(meta) = from_item::<ListMeta>(item) {
                    if meta.deleted {
                        continue;
                    }
                    let id = pk.trim_start_matches("LIST#").to_string();
                    catalog.lists.insert(
                        id.clone(),
                        WordList {
                            id,
                            title: meta.title,
                            owner: meta.owner,
                            entries: Vec::new(),
                        },
                    );
                }
            } else if pk.starts_with("LIST#") && sk.starts_with("POS#") {
                if let Ok(row) = from_item::<Membership>(item) {
                    let pos: u32 = sk.trim_start_matches("POS#").parse().unwrap_or(0);
                    membership.push((pk, pos, row.word_id));
                }
            } else if pk.starts_with("USER#") && sk == "QUEUE" {
                if let Ok(queue) = from_item::<QueueRecord>(item) {
                    catalog
                        .queue_versions
                        .insert(pk.trim_start_matches("USER#").to_string(), queue.version);
                    catalog
                        .queues
                        .insert(pk.trim_start_matches("USER#").to_string(), queue.list_ids);
                }
            }
        }
        for (pk, pos, word_id) in membership {
            let id = pk.trim_start_matches("LIST#");
            if let Some(list) = catalog.lists.get_mut(id) {
                list.entries.push((pos, word_id));
            }
        }
        for list in catalog.lists.values_mut() {
            list.entries.sort_by_key(|(pos, _)| *pos);
        }
        Ok(catalog)
    }
}

pub async fn refresh_catalog(store: &Store, catalog: &mut Catalog) -> ApiResult<()> {
    if store.catalog_version().await? != catalog.revision {
        *catalog = Catalog::load(store).await?;
    }
    Ok(())
}

pub async fn ensure_user(store: &Store, catalog: &mut Catalog, sub: &str) -> ApiResult<()> {
    for attempt in 0..16 {
        refresh_catalog(store, catalog).await?;
        let list_id = format!("my-{sub}");
        let mut writes = Vec::new();
        if !catalog.lists.contains_key(&list_id) {
            // A deliberately deleted personal list must not be resurrected by bootstrap.
            if store
                .get::<ListMeta>(&format!("LIST#{list_id}"), "META")
                .await?
                .is_none()
            {
                writes.push(store.transaction_put(&ListMeta {
                    pk: format!("LIST#{list_id}"),
                    sk: "META".into(),
                    title: "My list".into(),
                    owner: sub.into(),
                    deleted: false,
                })?);
            }
        }
        if !catalog.queues.contains_key(sub) {
            writes.push(store.transaction_put(&QueueRecord {
                pk: crate::records::user_pk(sub),
                sk: "QUEUE".into(),
                list_ids: vec![list_id, "hsk-1".into()],
                version: 0,
            })?);
        }
        if writes.is_empty() {
            return Ok(());
        }
        match store.catalog_transaction(catalog.revision, writes).await {
            Ok(()) => {
                *catalog = Catalog::load(store).await?;
                return Ok(());
            }
            Err(ApiError::Conflict(_)) => {
                // Another local process may have completed this same idempotent setup.
                tokio::time::sleep(std::time::Duration::from_millis(2 * (attempt + 1))).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(ApiError::conflict(
        "Local user setup changed repeatedly. Retry startup.",
    ))
}
