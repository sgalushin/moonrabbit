use std::collections::HashSet;

use chrono::{Datelike, Local, NaiveDate};
use serde_json::{json, Value};

use crate::cards::{load_cards, load_days, studied_ids, CardPhase, Direction};
use crate::catalog::Catalog;
use crate::error::ApiResult;
use crate::lists::queue_rows;
use crate::store::Store;

pub async fn dashboard(store: &Store, catalog: &Catalog, sub: &str) -> ApiResult<Value> {
    let cards = load_cards(store, sub).await?;
    let days = load_days(store, sub).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let due = cards
        .values()
        .filter(|card| {
            card.schedule.due <= now
                && card.schedule.state != CardPhase::New
                && card.schedule.reps > 0
        })
        .count();
    let recallable = cards
        .values()
        .filter(|card| {
            card.direction == Direction::Recognition && card.schedule.state == CardPhase::Review
        })
        .count();
    let forgotten = cards
        .values()
        .filter(|card| card.direction == Direction::Recognition && card.schedule.lapses > 0)
        .count();
    let studied = studied_ids(&cards);
    let characters_studied = studied
        .iter()
        .filter(|id| {
            catalog
                .words
                .get(*id)
                .is_some_and(|word| word.simplified.chars().count() == 1)
        })
        .count();
    let good: u32 = days.iter().map(|day| day.good).sum();
    let again: u32 = days.iter().map(|day| day.again).sum();
    let retention = (good + again > 0).then_some(good as f64 / (good + again) as f64);
    let studied_days: HashSet<String> = days
        .iter()
        .filter(|day| day.good + day.again > 0)
        .map(|day| day.sk.trim_start_matches("DAY#").to_string())
        .collect();
    let queue = queue_rows(catalog, sub, &studied);
    Ok(json!({
        "due": due,
        "recallable": recallable,
        "forgotten": forgotten,
        "charactersStudied": characters_studied,
        "retention": retention,
        "days": month_days(&studied_days),
        "queue": queue,
        "queueVersion": catalog.queue_versions.get(sub).copied().unwrap_or(0),
    }))
}
fn month_days(studied: &HashSet<String>) -> Vec<Value> {
    let today = Local::now().date_naive();
    let start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today);
    let mut days = Vec::new();
    let mut cursor = start;
    while cursor.month() == start.month() {
        let key = cursor.format("%Y-%m-%d").to_string();
        days.push(json!({ "date": key, "studied": studied.contains(&key) }));
        cursor = cursor.succ_opt().unwrap_or(cursor);
        if cursor == start {
            break;
        }
    }
    days
}
