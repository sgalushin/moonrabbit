use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::{watch, Mutex};
use unicode_normalization::UnicodeNormalization;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Fish Audio's Mandarin voice Bingbing.
const VOICE: &str = "e98fa6cdad6946bf8d9bb8f9cb8c2532";
type Outcome = Option<ApiResult<Bytes>>;

#[derive(Clone, Default)]
pub struct SpeechFlights {
    inner: Arc<Mutex<HashMap<String, watch::Receiver<Outcome>>>>,
}

pub fn cache_key(model: &str, text: &str, input: &str, word_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(text.as_bytes());
    digest.update(b"\n");
    digest.update(input.as_bytes());
    format!(
        "speech/v2/{model}/{VOICE}/{word_id}/{}.mp3",
        hex::encode(digest.finalize())
    )
}

pub async fn speak(state: &AppState, text: &str, pinyin: &str, word_id: &str) -> ApiResult<Bytes> {
    let input = pronunciation_input(pinyin)?;
    let key = cache_key(&state.config.speech_model, text, &input, word_id);
    let (receiver, leader) = state.speech.begin(&key).await;
    if let Some(sender) = leader {
        let state = state.clone();
        // A disconnected initiating request must not cancel work and strand its followers.
        tokio::spawn(async move {
            let outcome = cached_or_synthesize(&state, &key, &input).await;
            sender.send_replace(Some(outcome));
            state.speech.finish(&key).await;
        });
    }
    flight_result(receiver).await
}

impl SpeechFlights {
    async fn begin(&self, key: &str) -> (watch::Receiver<Outcome>, Option<watch::Sender<Outcome>>) {
        let mut flights = self.inner.lock().await;
        if let Some(existing) = flights.get(key) {
            if existing.has_changed().is_ok() {
                return (existing.clone(), None);
            }
        }
        let (sender, receiver) = watch::channel(None);
        flights.insert(key.to_string(), receiver.clone());
        (receiver, Some(sender))
    }

    async fn finish(&self, key: &str) {
        self.inner.lock().await.remove(key);
    }
}

async fn flight_result(mut receiver: watch::Receiver<Outcome>) -> ApiResult<Bytes> {
    loop {
        // Completion is persistent, including when it predates this follower's first poll.
        if let Some(outcome) = receiver.borrow_and_update().clone() {
            return outcome;
        }
        receiver
            .changed()
            .await
            .map_err(|_| ApiError::gateway("This word could not be voiced."))?;
    }
}

async fn cached_or_synthesize(state: &AppState, key: &str, input: &str) -> ApiResult<Bytes> {
    if let Some(cached) = state.blobs.get_bytes(key).await? {
        return Ok(cached);
    }
    let api_key = state.config.provider_key.as_deref().ok_or_else(|| {
        ApiError::unavailable("OPENROUTER_API_KEY is not set. Speech is unavailable.")
    })?;
    let audio = synthesize(&state.http, &state.config.speech_model, api_key, input).await?;
    state
        .blobs
        .put_bytes(key, audio.clone(), "audio/mpeg")
        .await?;
    Ok(audio)
}

pub(crate) fn sentence_punctuation(ch: char) -> bool {
    matches!(
        ch,
        ',' | '.'
            | '!'
            | '?'
            | ';'
            | ':'
            | '\''
            | '"'
            | '('
            | ')'
            | '-'
            | '，'
            | '。'
            | '！'
            | '？'
            | '；'
            | '：'
            | '、'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '（'
            | '）'
            | '《'
            | '》'
            | '…'
            | '—'
    )
}

/// Fish S2.1 accepts one tone-number pinyin syllable per phoneme tag, not ordinary
/// spoken pinyin: https://docs.fish.audio/developer-guide/core-features/fine-grained-control/chinese
/// OpenRouter forwards string `input` to Fish's TTS endpoint:
/// https://openrouter.ai/docs/guides/overview/multimodal/tts
pub(crate) fn pronunciation_input(pinyin: &str) -> ApiResult<String> {
    let mut input = String::new();
    let mut has_syllable = false;
    for token in pinyin.split_inclusive(|ch: char| ch.is_whitespace() || sentence_punctuation(ch)) {
        let punctuation = token.chars().last().filter(|ch| sentence_punctuation(*ch));
        let syllable = token.trim_end_matches(sentence_punctuation).trim();
        if syllable.is_empty() {
            if let Some(ch) = punctuation {
                input.push(ch);
            }
            continue;
        }
        let mut letters = String::new();
        let mut tone = None;
        for ch in syllable.to_lowercase().nfd() {
            match ch {
                '\u{0304}' | '\u{0301}' | '\u{030c}' | '\u{0300}' => {
                    let mark = match ch {
                        '\u{0304}' => 1,
                        '\u{0301}' => 2,
                        '\u{030c}' => 3,
                        _ => 4,
                    };
                    if tone.replace(mark).is_some() {
                        return Err(ApiError::bad(
                            "This reading has multiple tone marks in a syllable.",
                        ));
                    }
                }
                '\u{0308}' if letters.ends_with('u') => {
                    letters.pop();
                    // `to_tone3` in Fish's documented training helper uses v for ü.
                    letters.push('v');
                }
                '1'..='5' if tone.is_none() && syllable.ends_with(ch) => {
                    tone = Some(ch as u8 - b'0');
                }
                'a'..='z' => letters.push(ch),
                _ => {
                    return Err(ApiError::bad(
                        "This word's pinyin is not supported by Chinese phoneme control.",
                    ))
                }
            }
        }
        if letters.is_empty() {
            return Err(ApiError::bad("This word's pinyin has an empty syllable."));
        }
        input.push_str("<|phoneme_start|>");
        input.push_str(&letters);
        input.push(char::from(b'0' + tone.unwrap_or(5)));
        input.push_str("<|phoneme_end|>");
        has_syllable = true;
        if let Some(ch) = punctuation {
            input.push(ch);
        }
    }
    if !has_syllable {
        return Err(ApiError::bad("This word has no pronunciation configured."));
    }
    Ok(input)
}

async fn synthesize(
    http: &reqwest::Client,
    model: &str,
    api_key: &str,
    input: &str,
) -> ApiResult<Bytes> {
    let response = http
        .post("https://openrouter.ai/api/v1/audio/speech")
        .bearer_auth(api_key)
        .header(reqwest::header::ACCEPT, "audio/mpeg")
        .json(&json!({
            "model": model,
            "input": input,
            "voice": VOICE,
            "response_format": "mp3"
        }))
        .send()
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "speech request failed");
            ApiError::gateway("Speech could not be reached.")
        })?;
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::gateway(provider_message(&body)));
    }
    response
        .bytes()
        .await
        .map_err(|_| ApiError::gateway("Speech did not return audio."))
}

fn provider_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|parsed| {
            parsed
                .pointer("/error/message")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| "This word could not be voiced.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completion_before_follower_poll_is_retained() {
        let flights = SpeechFlights::default();
        let (leader, sender) = flights.begin("word").await;
        let (follower, second_sender) = flights.begin("word").await;
        assert!(second_sender.is_none());
        let audio = Bytes::from_static(b"clip");
        sender.unwrap().send_replace(Some(Ok(audio.clone())));
        flights.finish("word").await;
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(1), flight_result(follower))
                .await
                .expect("a follower must not miss an already-published completion");
        assert_eq!(result.unwrap(), audio);
        assert_eq!(flight_result(leader).await.unwrap(), audio);
    }

    #[tokio::test]
    async fn abandoned_work_reports_error_instead_of_hanging_followers() {
        let flights = SpeechFlights::default();
        let (_, sender) = flights.begin("word").await;
        let (follower, _) = flights.begin("word").await;
        drop(sender);
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(1), flight_result(follower))
                .await
                .expect("abandoned work must not hang followers");
        assert!(result.is_err());
    }

    #[test]
    fn polyphonic_readings_use_distinct_provider_inputs_and_cache_keys() {
        let chang = pronunciation_input("cháng").unwrap();
        let zhang = pronunciation_input("zhǎng").unwrap();
        assert_eq!(chang, "<|phoneme_start|>chang2<|phoneme_end|>");
        assert_eq!(zhang, "<|phoneme_start|>zhang3<|phoneme_end|>");
        assert_ne!(
            cache_key("fish-audio/s2.1-pro", "长", &chang, "word"),
            cache_key("fish-audio/s2.1-pro", "长", &zhang, "word")
        );
        assert_eq!(
            pronunciation_input("nǚ zi").unwrap(),
            "<|phoneme_start|>nv3<|phoneme_end|><|phoneme_start|>zi5<|phoneme_end|>"
        );
        assert_eq!(
            pronunciation_input("chong2 qing4").unwrap(),
            "<|phoneme_start|>chong2<|phoneme_end|><|phoneme_start|>qing4<|phoneme_end|>"
        );
        assert!(pronunciation_input("<|phoneme_end|>").is_err());
        assert_eq!(
            pronunciation_input("Nǐ hǎo，shì ma?").unwrap(),
            "<|phoneme_start|>ni3<|phoneme_end|><|phoneme_start|>hao3<|phoneme_end|>，<|phoneme_start|>shi4<|phoneme_end|><|phoneme_start|>ma5<|phoneme_end|>?"
        );
        assert!(pronunciation_input("？！").is_err());
        assert_ne!(
            cache_key("fish-audio/s2.1-pro", "你好", &chang, "word/sentence-one"),
            cache_key("fish-audio/s2.1-pro", "你好", &chang, "word/sentence-two")
        );
    }
}
