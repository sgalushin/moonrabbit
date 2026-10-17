use fsrs::{MemoryState, DEFAULT_PARAMETERS, FSRS};

use crate::cards::{CardPhase, CardState, Rating};
use crate::error::{ApiError, ApiResult};

const MINUTE: i64 = 60_000;
const DAY: i64 = 86_400_000;
const MAX_DAYS: f32 = 36_500.0;

pub fn grade(card: &CardState, rating: Rating, reviewed_at: i64) -> ApiResult<CardState> {
    let state = card.state;
    if reviewed_at < 0 || card.last_review < 0 || reviewed_at < card.last_review {
        return Err(ApiError::bad(
            "review timestamp must be nonnegative and chronological",
        ));
    }
    if matches!(state, CardPhase::Learning) && card.step > 2 {
        return Err(ApiError::bad("unknown learning step"));
    }
    let reps = card
        .reps
        .checked_add(1)
        .ok_or_else(|| ApiError::bad("repetition count overflow"))?;
    let version = card
        .version
        .checked_add(1)
        .ok_or_else(|| ApiError::bad("card version overflow"))?;
    let lapses = match (state, rating) {
        (CardPhase::Review, Rating::Again) => card
            .lapses
            .checked_add(1)
            .ok_or_else(|| ApiError::bad("lapse count overflow"))?,
        _ => card.lapses,
    };
    let forecast = forecast(card, state, reviewed_at)?;
    let item = match rating {
        Rating::Again => &forecast.again,
        Rating::Good => &forecast.good,
    };
    let (next_state, step, delay) = match (state, rating) {
        (CardPhase::New, Rating::Again) | (CardPhase::Learning, Rating::Again) => {
            (CardPhase::Learning, 0, MINUTE)
        }
        (CardPhase::New, Rating::Good) => (CardPhase::Learning, 1, MINUTE),
        (CardPhase::Learning, Rating::Good) => match card.step {
            0 => (CardPhase::Learning, 1, MINUTE),
            1 => (CardPhase::Learning, 2, 10 * MINUTE),
            2 => (
                CardPhase::Review,
                0,
                interval_ms(item.interval, reviewed_at)?,
            ),
            _ => unreachable!("learning step validated above"),
        },
        (CardPhase::Review, Rating::Again) | (CardPhase::Relearning, Rating::Again) => {
            (CardPhase::Relearning, 0, 10 * MINUTE)
        }
        (CardPhase::Review, Rating::Good) | (CardPhase::Relearning, Rating::Good) => (
            CardPhase::Review,
            0,
            interval_ms(item.interval, reviewed_at)?,
        ),
    };
    let due = reviewed_at
        .checked_add(delay)
        .ok_or_else(|| ApiError::bad("review due timestamp overflow"))?;
    let mut next = *card;
    next.state = next_state;
    next.step = step;
    next.stability = item.memory.stability;
    next.difficulty = item.memory.difficulty;
    next.due = due;
    next.last_review = reviewed_at;
    next.reps = reps;
    next.lapses = lapses;
    next.version = version;
    Ok(next)
}

fn forecast(card: &CardState, state: CardPhase, reviewed_at: i64) -> ApiResult<fsrs::NextStates> {
    let memory = if card.stability == 0.0 && card.difficulty == 0.0 {
        // Existing New/Learning records predate per-answer memory updates.
        if matches!(state, CardPhase::Review | CardPhase::Relearning) {
            return Err(ApiError::bad("review card is missing FSRS memory"));
        }
        None
    } else {
        if !card.stability.is_finite()
            || card.stability <= 0.0
            || !card.difficulty.is_finite()
            || !(1.0..=10.0).contains(&card.difficulty)
        {
            return Err(ApiError::bad("invalid FSRS memory"));
        }
        Some(MemoryState {
            stability: card.stability,
            difficulty: card.difficulty,
        })
    };
    let elapsed = if memory.is_some() {
        let elapsed_ms = reviewed_at
            .checked_sub(card.last_review)
            .ok_or_else(|| ApiError::bad("elapsed review time overflow"))?;
        u32::try_from(elapsed_ms / DAY)
            .map_err(|_| ApiError::bad("elapsed review time exceeds FSRS range"))?
    } else {
        0
    };
    thread_local! {
        static MODEL: Result<FSRS, fsrs::FSRSError> = FSRS::new(&DEFAULT_PARAMETERS);
    }
    MODEL.with(|model| {
        let model = model.as_ref().map_err(|error| {
            tracing::error!(error = ?error, "FSRS initialization failed");
            ApiError::unavailable("FSRS initialization failed")
        })?;
        model.next_states(memory, 0.9, elapsed).map_err(|error| {
            tracing::error!(error = ?error, "FSRS forecast failed");
            ApiError::unavailable("FSRS forecast failed")
        })
    })
}

fn interval_ms(days: f32, reviewed_at: i64) -> ApiResult<i64> {
    if !days.is_finite() || days <= 0.0 {
        tracing::error!(days, "FSRS returned an invalid interval");
        return Err(ApiError::unavailable("FSRS returned an invalid interval"));
    }
    let days = days.clamp(1.0, MAX_DAYS);
    Ok(fuzz((days as f64 * DAY as f64) as i64, reviewed_at))
}

fn fuzz(interval: i64, seed: i64) -> i64 {
    if interval < DAY {
        return interval;
    }
    let unit = (seed.unsigned_abs() % 1000) as f64 / 1000.0;
    let scale = 0.95 + unit * 0.10;
    let fuzzed = (interval as f64 * scale) as i64;
    fuzzed.clamp(DAY, (MAX_DAYS as f64 * DAY as f64) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fsrs::ItemState;

    fn expected_item(card: &CardState, rating: Rating, reviewed_at: i64) -> ItemState {
        let memory = (card.stability > 0.0).then_some(MemoryState {
            stability: card.stability,
            difficulty: card.difficulty,
        });
        let elapsed = if memory.is_some() {
            ((reviewed_at - card.last_review) / DAY) as u32
        } else {
            0
        };
        let states = FSRS::new(&DEFAULT_PARAMETERS)
            .unwrap()
            .next_states(memory, 0.9, elapsed)
            .unwrap();
        match rating {
            Rating::Again => states.again,
            Rating::Good => states.good,
        }
    }

    fn assert_memory(card: &CardState, item: &ItemState) {
        assert!((card.stability - item.memory.stability).abs() < 0.00001);
        assert!((card.difficulty - item.memory.difficulty).abs() < 0.00001);
    }

    fn mature_card() -> CardState {
        CardState {
            state: CardPhase::Review,
            stability: 30.0,
            difficulty: 5.0,
            last_review: 7 * DAY,
            due: 37 * DAY,
            reps: 10,
            version: 10,
            lapses: 2,
            ..CardState::fresh()
        }
    }

    #[test]
    fn again_on_a_new_card_is_not_a_lapse_and_initializes_memory() {
        let fresh = CardState::fresh();
        let card = grade(&fresh, Rating::Again, 1_000).unwrap();
        assert_eq!(card.state, CardPhase::Learning);
        assert_eq!(
            (card.step, card.lapses, card.reps, card.version),
            (0, 0, 1, 1)
        );
        assert_eq!(card.due, 1_000 + MINUTE);
        assert_memory(&card, &expected_item(&fresh, Rating::Again, 1_000));
        assert!(card.stability > 0.0);
    }

    #[test]
    fn every_learning_answer_updates_same_day_memory_while_steps_control_due() {
        let fresh = CardState::fresh();
        let first = grade(&fresh, Rating::Good, 0).unwrap();
        assert_memory(&first, &expected_item(&fresh, Rating::Good, 0));
        assert_eq!(
            (first.state, first.step, first.due),
            (CardPhase::Learning, 1, MINUTE)
        );
        let second = grade(&first, Rating::Good, first.due).unwrap();
        assert_memory(&second, &expected_item(&first, Rating::Good, first.due));
        assert_eq!(
            (second.state, second.step, second.due),
            (CardPhase::Learning, 2, first.due + 10 * MINUTE)
        );
        assert_ne!(second.difficulty, first.difficulty);
        let expected = expected_item(&second, Rating::Good, second.due);
        let third = grade(&second, Rating::Good, second.due).unwrap();
        assert_memory(&third, &expected);
        assert_eq!((third.state, third.step), (CardPhase::Review, 0));
        assert_eq!(
            third.due,
            second.due + interval_ms(expected.interval, second.due).unwrap()
        );
    }

    #[test]
    fn repeated_learning_failures_affect_graduation_memory() {
        let easy = grade(&CardState::fresh(), Rating::Good, 0).unwrap();
        let easy = grade(&easy, Rating::Good, easy.due).unwrap();
        let easy = grade(&easy, Rating::Good, easy.due).unwrap();

        let mut difficult = grade(&CardState::fresh(), Rating::Again, 0).unwrap();
        for _ in 0..3 {
            let at = difficult.due;
            let expected = expected_item(&difficult, Rating::Again, at);
            difficult = grade(&difficult, Rating::Again, at).unwrap();
            assert_memory(&difficult, &expected);
            assert_eq!(difficult.lapses, 0);
        }
        for _ in 0..3 {
            difficult = grade(&difficult, Rating::Good, difficult.due).unwrap();
        }
        assert_eq!(difficult.state, CardPhase::Review);
        assert!(difficult.difficulty > easy.difficulty);
        assert_ne!(difficult.stability, easy.stability);
    }

    #[test]
    fn mature_good_remains_review_and_uses_actual_elapsed_days() {
        let card = mature_card();
        let at = card.due;
        let expected = expected_item(&card, Rating::Good, at);
        let same_day = expected_item(&card, Rating::Good, card.last_review);
        let next = grade(&card, Rating::Good, at).unwrap();
        assert_eq!((next.state, next.step), (CardPhase::Review, 0));
        assert_eq!((next.reps, next.version, next.lapses), (11, 11, 2));
        assert_eq!(next.last_review, at);
        assert_memory(&next, &expected);
        assert_ne!(next.stability, same_day.memory.stability);
        assert_eq!(next.due, at + interval_ms(expected.interval, at).unwrap());
        assert!(next.due - at >= DAY);
    }

    #[test]
    fn zero_timestamp_is_a_real_previous_memory_update() {
        let card = CardState {
            last_review: 0,
            ..mature_card()
        };
        let at = 30 * DAY;
        let next = grade(&card, Rating::Good, at).unwrap();
        assert_memory(&next, &expected_item(&card, Rating::Good, at));
    }

    #[test]
    fn lapse_is_counted_once_and_relearning_good_uses_a_new_forecast_once() {
        let card = mature_card();
        let at = card.due + 999;
        let expected = expected_item(&card, Rating::Again, at);
        let lapsed = grade(&card, Rating::Again, at).unwrap();
        assert_memory(&lapsed, &expected);
        assert_eq!(
            (lapsed.state, lapsed.lapses, lapsed.step),
            (CardPhase::Relearning, 3, 0)
        );
        assert_eq!(lapsed.due, at + 10 * MINUTE);

        let expected_retry = expected_item(&lapsed, Rating::Again, lapsed.due);
        let retry = grade(&lapsed, Rating::Again, lapsed.due).unwrap();
        assert_memory(&retry, &expected_retry);
        assert_eq!(retry.lapses, lapsed.lapses);
        assert_eq!(retry.due, lapsed.due + 10 * MINUTE);
        let expected_good = expected_item(&retry, Rating::Good, retry.due);
        let restored = grade(&retry, Rating::Good, retry.due).unwrap();
        assert_memory(&restored, &expected_good);
        assert_eq!((restored.state, restored.step), (CardPhase::Review, 0));
        assert_eq!(restored.lapses, 3);
        assert_eq!(
            restored.due,
            retry.due + interval_ms(expected_good.interval, retry.due).unwrap()
        );
    }

    #[test]
    fn legacy_learning_cards_without_memory_initialize_on_the_next_answer() {
        let card = CardState {
            state: CardPhase::Learning,
            step: 2,
            reps: 2,
            last_review: MINUTE,
            ..CardState::fresh()
        };
        let next = grade(&card, Rating::Good, 11 * MINUTE).unwrap();
        assert_eq!(next.state, CardPhase::Review);
        assert_memory(&next, &expected_item(&card, Rating::Good, 11 * MINUTE));
    }

    #[test]
    fn unknown_steps_and_invalid_memory_are_rejected() {
        let step = CardState {
            state: CardPhase::Learning,
            step: 3,
            ..CardState::fresh()
        };
        assert!(grade(&step, Rating::Good, 0).is_err());
        let missing = CardState {
            state: CardPhase::Review,
            ..CardState::fresh()
        };
        assert!(grade(&missing, Rating::Good, 0).is_err());
        for stability in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            let card = CardState {
                stability,
                ..mature_card()
            };
            assert!(grade(&card, Rating::Good, card.due).is_err());
        }
        for difficulty in [f32::NAN, f32::INFINITY, 0.0, 11.0] {
            let card = CardState {
                difficulty,
                ..mature_card()
            };
            assert!(grade(&card, Rating::Good, card.due).is_err());
        }
        assert!(interval_ms(f32::NAN, 0).is_err());
        assert!(interval_ms(f32::INFINITY, 0).is_err());
    }

    #[test]
    fn timestamp_boundaries_and_counter_overflow_are_rejected_without_mutation() {
        let card = mature_card();
        assert!(grade(&card, Rating::Good, -1).is_err());
        assert!(grade(&card, Rating::Good, card.last_review - 1).is_err());
        assert!(grade(&card, Rating::Good, i64::MIN).is_err());
        assert!(grade(&card, Rating::Good, i64::MAX).is_err());
        let invalid_last = CardState {
            last_review: i64::MIN,
            ..card
        };
        assert!(grade(&invalid_last, Rating::Good, 0).is_err());
        let near_limit = i64::MAX - MINUTE;
        let fresh = CardState::fresh();
        assert_eq!(
            grade(&fresh, Rating::Good, near_limit).unwrap().due,
            i64::MAX
        );
        assert!(grade(&fresh, Rating::Good, near_limit + 1).is_err());
        let review_near_limit = CardState {
            last_review: i64::MAX - DAY,
            ..card
        };
        assert!(grade(&review_near_limit, Rating::Good, i64::MAX).is_err());
        for overflowing in [
            CardState {
                reps: u32::MAX,
                ..card
            },
            CardState {
                version: u32::MAX,
                ..card
            },
            CardState {
                lapses: u32::MAX,
                ..card
            },
        ] {
            assert!(grade(&overflowing, Rating::Again, overflowing.due).is_err());
        }
        assert_eq!(
            (card.state, card.reps, card.version, card.lapses),
            (CardPhase::Review, 10, 10, 2)
        );
    }
}
