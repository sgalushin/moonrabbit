use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::records::{user_pk, CardRecord, DayRecord};
use crate::store::Store;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Recognition,
    Production,
}

impl Direction {
    pub fn key(self) -> &'static str {
        match self {
            Self::Recognition => "recognition",
            Self::Production => "production",
        }
    }
}

impl TryFrom<&str> for Direction {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "recognition" => Ok(Self::Recognition),
            "production" => Ok(Self::Production),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rating {
    Again,
    Good,
}

impl Rating {
    pub fn key(self) -> &'static str {
        match self {
            Self::Again => "again",
            Self::Good => "good",
        }
    }
}

impl TryFrom<&str> for Rating {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "again" => Ok(Self::Again),
            "good" => Ok(Self::Good),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardPhase {
    New,
    Learning,
    Review,
    Relearning,
}

impl CardPhase {
    pub fn key(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Learning => "learning",
            Self::Review => "review",
            Self::Relearning => "relearning",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardState {
    pub state: CardPhase,
    pub step: u32,
    pub stability: f32,
    pub difficulty: f32,
    pub due: i64,
    pub last_review: i64,
    pub reps: u32,
    pub lapses: u32,
    pub version: u32,
}

impl CardState {
    pub fn fresh() -> Self {
        Self {
            state: CardPhase::New,
            step: 0,
            stability: 0.0,
            difficulty: 0.0,
            due: 0,
            last_review: 0,
            reps: 0,
            lapses: 0,
            version: 0,
        }
    }
}

pub(crate) fn studied_ids(cards: &HashMap<String, CardRecord>) -> HashSet<String> {
    cards
        .values()
        .filter(|card| card.schedule.reps > 0)
        .map(|card| card.word_id.clone())
        .collect()
}

pub(crate) async fn load_cards(store: &Store, sub: &str) -> ApiResult<HashMap<String, CardRecord>> {
    let mut cards = HashMap::new();
    for item in store.query_prefix(&user_pk(sub), "CARD#").await? {
        if let Ok(record) = serde_dynamo::aws_sdk_dynamodb_1::from_item::<CardRecord>(item) {
            cards.insert(record.sk.clone(), record);
        }
    }
    Ok(cards)
}

pub(crate) async fn load_days(store: &Store, sub: &str) -> ApiResult<Vec<DayRecord>> {
    let mut days = Vec::new();
    for item in store.query_prefix(&user_pk(sub), "DAY#").await? {
        if let Ok(day) = serde_dynamo::aws_sdk_dynamodb_1::from_item(item) {
            days.push(day);
        }
    }
    Ok(days)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn either_direction_counts_as_studied_without_double_counting_words() {
        let card = |word: &str, direction: Direction, reps| CardRecord {
            pk: user_pk("learner"),
            sk: crate::records::card_sk(word, direction),
            word_id: word.to_owned(),
            direction,
            schedule: CardState {
                state: CardPhase::Learning,
                reps,
                ..CardState::fresh()
            },
        };
        let cards = [
            card("production-only", Direction::Production, 1),
            card("both", Direction::Recognition, 3),
            card("both", Direction::Production, 2),
            card("untouched", Direction::Recognition, 0),
        ]
        .into_iter()
        .map(|card| (card.sk.clone(), card))
        .collect();
        assert_eq!(
            studied_ids(&cards),
            HashSet::from(["production-only".to_owned(), "both".to_owned()]),
        );
    }
}
