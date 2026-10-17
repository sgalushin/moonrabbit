use std::collections::HashMap;

use aws_sdk_dynamodb::types::{
    AttributeValue, ConditionCheck, Delete, Put, PutRequest, TransactWriteItem, Update,
    WriteRequest,
};
use aws_sdk_dynamodb::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{storage, ApiError, ApiResult};
use crate::records::{CardRecord, CatalogRevision, GradeRecord};

#[derive(Clone)]
pub struct Store {
    client: Client,
    table: String,
}

impl Store {
    pub fn new(client: Client, table: String) -> Self {
        Self { client, table }
    }

    pub async fn put<T: Serialize>(&self, value: &T) -> ApiResult<()> {
        let item = serde_dynamo::aws_sdk_dynamodb_1::to_item(value)
            .map_err(|error| storage(error, "That could not be saved."))?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .send()
            .await
            .map_err(|error| storage(error, "That could not be saved."))?;
        Ok(())
    }

    pub async fn get<T: DeserializeOwned>(&self, pk: &str, sk: &str) -> ApiResult<Option<T>> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(pk.to_string()))
            .key("sk", AttributeValue::S(sk.to_string()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| storage(error, "That could not be loaded."))?;
        match output.item {
            Some(item) => Ok(Some(
                serde_dynamo::aws_sdk_dynamodb_1::from_item(item)
                    .map_err(|error| storage(error, "That could not be loaded."))?,
            )),
            None => Ok(None),
        }
    }

    pub async fn query_prefix(
        &self,
        pk: &str,
        sk_prefix: &str,
    ) -> ApiResult<Vec<HashMap<String, AttributeValue>>> {
        let mut items = Vec::new();
        let mut start = None;
        loop {
            let mut request = self
                .client
                .query()
                .table_name(&self.table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :sk)")
                .expression_attribute_values(":pk", AttributeValue::S(pk.to_string()))
                .expression_attribute_values(":sk", AttributeValue::S(sk_prefix.to_string()));
            request = request.consistent_read(true);
            request = request.set_exclusive_start_key(start.take());
            let output = request
                .send()
                .await
                .map_err(|error| storage(error, "That could not be loaded."))?;
            items.extend(output.items.unwrap_or_default());
            match output.last_evaluated_key {
                Some(key) if !key.is_empty() => start = Some(key),
                _ => break,
            }
        }
        Ok(items)
    }

    pub async fn scan(&self) -> ApiResult<Vec<HashMap<String, AttributeValue>>> {
        let mut items = Vec::new();
        let mut start = None;
        loop {
            let mut request = self
                .client
                .scan()
                .table_name(&self.table)
                .consistent_read(true);
            request = request.set_exclusive_start_key(start.take());
            let output = request
                .send()
                .await
                .map_err(|error| storage(error, "The catalog could not be loaded."))?;
            items.extend(output.items.unwrap_or_default());
            match output.last_evaluated_key {
                Some(key) if !key.is_empty() => start = Some(key),
                _ => break,
            }
        }
        Ok(items)
    }

    pub async fn batch_put<T: Serialize>(&self, values: &[T]) -> ApiResult<()> {
        if values.is_empty() {
            return Ok(());
        }
        let mut writes = Vec::new();
        for value in values {
            let item = serde_dynamo::aws_sdk_dynamodb_1::to_item(value)
                .map_err(|error| storage(error, "The catalog could not be saved."))?;
            writes.push(
                WriteRequest::builder()
                    .put_request(
                        PutRequest::builder()
                            .set_item(Some(item))
                            .build()
                            .map_err(|error| storage(error, "The catalog could not be saved."))?,
                    )
                    .build(),
            );
        }
        self.flush_writes(writes).await
    }

    async fn flush_writes(&self, writes: Vec<WriteRequest>) -> ApiResult<()> {
        let mut writes = writes.into_iter();
        loop {
            let mut pending: Vec<_> = writes.by_ref().take(25).collect();
            if pending.is_empty() {
                break;
            }
            for attempt in 0..20 {
                if pending.is_empty() {
                    break;
                }
                if attempt > 0 {
                    let shift = (attempt - 1).min(6);
                    tokio::time::sleep(std::time::Duration::from_millis(50 << shift)).await;
                }
                let output = self
                    .client
                    .batch_write_item()
                    .request_items(&self.table, pending)
                    .send()
                    .await
                    .map_err(|error| storage(error, "The catalog could not be saved."))?;
                pending = output
                    .unprocessed_items
                    .and_then(|mut items| items.remove(&self.table))
                    .unwrap_or_default();
            }
            if !pending.is_empty() {
                return Err(storage(
                    "unprocessed writes",
                    "The catalog could not be saved.",
                ));
            }
        }
        Ok(())
    }

    fn serialized_put<T: Serialize>(&self, value: &T) -> ApiResult<Put> {
        let item = serde_dynamo::aws_sdk_dynamodb_1::to_item(value)
            .map_err(|error| storage(error, "That could not be saved."))?;
        Put::builder()
            .table_name(&self.table)
            .set_item(Some(item))
            .build()
            .map_err(|error| storage(error, "That could not be saved."))
    }

    pub fn transaction_put<T: Serialize>(&self, value: &T) -> ApiResult<TransactWriteItem> {
        Ok(TransactWriteItem::builder()
            .put(self.serialized_put(value)?)
            .build())
    }

    pub fn transaction_delete(&self, pk: &str, sk: &str) -> ApiResult<TransactWriteItem> {
        Ok(TransactWriteItem::builder()
            .delete(
                Delete::builder()
                    .table_name(&self.table)
                    .key("pk", AttributeValue::S(pk.into()))
                    .key("sk", AttributeValue::S(sk.into()))
                    .build()
                    .map_err(|error| storage(error, "That could not be removed."))?,
            )
            .build())
    }

    // Only conditional/version conflicts are retryable; storage failures are never
    // misreported as successful writes.
    pub async fn transact(&self, writes: Vec<TransactWriteItem>) -> ApiResult<bool> {
        match self
            .client
            .transact_write_items()
            .set_transact_items(Some(writes))
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(error) => {
                if let Some(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(cancelled)) = error.as_service_error() {
                    if cancelled.cancellation_reasons().iter().any(|reason|
                        matches!(reason.code(), Some("ConditionalCheckFailed" | "TransactionConflict"))) {
                        return Ok(false);
                    }
                }
                Err(storage(error, "That could not be saved."))
            }
        }
    }

    pub async fn catalog_version(&self) -> ApiResult<u64> {
        Ok(self
            .get::<CatalogRevision>("CATALOG", "REV")
            .await?
            .map(|revision| revision.version)
            .unwrap_or(0))
    }

    pub async fn catalog_transaction(
        &self,
        expected: u64,
        mut writes: Vec<TransactWriteItem>,
    ) -> ApiResult<()> {
        let next = expected
            .checked_add(1)
            .ok_or_else(|| ApiError::unavailable("Catalog version exhausted."))?;
        let condition = if expected == 0 {
            "attribute_not_exists(#version) OR #version = :expected"
        } else {
            "#version = :expected"
        };
        let update = Update::builder()
            .table_name(&self.table)
            .key("pk", AttributeValue::S("CATALOG".into()))
            .key("sk", AttributeValue::S("REV".into()))
            .update_expression("SET #version = :next")
            .condition_expression(condition)
            .expression_attribute_names("#version", "version")
            .expression_attribute_values(":expected", AttributeValue::N(expected.to_string()))
            .expression_attribute_values(":next", AttributeValue::N(next.to_string()))
            .build()
            .map_err(|error| storage(error, "That could not be saved."))?;
        writes.push(TransactWriteItem::builder().update(update).build());
        if self.transact(writes).await? {
            Ok(())
        } else {
            Err(ApiError::conflict(
                "The catalog changed. Reload and try again.",
            ))
        }
    }

    pub async fn grade_transaction(
        &self,
        event: &GradeRecord,
        card: Option<(&CardRecord, Option<u32>)>,
        stale_card: Option<&CardRecord>,
        day_sk: &str,
    ) -> ApiResult<bool> {
        let mut event_put = self.serialized_put(event)?;
        event_put.condition_expression = Some("attribute_not_exists(pk)".into());
        let mut writes = vec![TransactWriteItem::builder().put(event_put).build()];
        if let Some(stale_card) = stale_card {
            let condition = ConditionCheck::builder()
                .table_name(&self.table)
                .key("pk", AttributeValue::S(stale_card.pk.clone()))
                .key("sk", AttributeValue::S(stale_card.sk.clone()))
                .condition_expression("#version = :expected")
                .expression_attribute_names("#version", "version")
                .expression_attribute_values(
                    ":expected",
                    AttributeValue::N(stale_card.schedule.version.to_string()),
                )
                .build()
                .map_err(|error| storage(error, "That could not be saved."))?;
            writes.push(
                TransactWriteItem::builder()
                    .condition_check(condition)
                    .build(),
            );
        }
        if let Some((card, expected)) = card {
            let mut card_put = self.serialized_put(card)?;
            card_put.condition_expression = Some(
                if expected.is_some() {
                    "#version = :expected"
                } else {
                    "attribute_not_exists(pk)"
                }
                .into(),
            );
            if let Some(version) = expected {
                card_put.expression_attribute_names =
                    Some(HashMap::from([("#version".into(), "version".into())]));
                card_put.expression_attribute_values = Some(HashMap::from([(
                    ":expected".into(),
                    AttributeValue::N(version.to_string()),
                )]));
            }
            writes.push(TransactWriteItem::builder().put(card_put).build());
            let update = Update::builder().table_name(&self.table)
                .key("pk", AttributeValue::S(event.pk.clone()))
                .key("sk", AttributeValue::S(day_sk.into()))
                .update_expression("SET #good = if_not_exists(#good, :zero) + :good, #again = if_not_exists(#again, :zero) + :again")
                .expression_attribute_names("#good", "good")
                .expression_attribute_names("#again", "again")
                .expression_attribute_values(":zero", AttributeValue::N("0".into()))
                .expression_attribute_values(":good", AttributeValue::N(if event.rating == "good" { "1" } else { "0" }.into()))
                .expression_attribute_values(":again", AttributeValue::N(if event.rating == "again" { "1" } else { "0" }.into()))
                .build().map_err(|error| storage(error, "That could not be saved."))?;
            writes.push(TransactWriteItem::builder().update(update).build());
        }
        self.transact(writes).await
    }
}

pub fn string_attr(item: &HashMap<String, AttributeValue>, key: &str) -> String {
    item.get(key)
        .and_then(|value| value.as_s().ok())
        .cloned()
        .unwrap_or_default()
}
