mod auth;
mod blobs;
mod cards;
mod catalog;
mod config;
mod dashboard;
mod dictionary;
mod enrichment;
mod error;
mod lists;
mod pinyin;
mod records;
mod schedule;
mod seed;
mod speech;
mod state;
mod store;
mod study;

use axum::extract::{DefaultBodyLimit, Json, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use crate::auth::User;
use crate::cards::Direction;
use crate::catalog::Catalog;
use crate::config::Config;
use crate::error::ApiResult;
use crate::lists::WordInput;
use crate::state::{AppState, Runtime};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .without_time()
        .try_init();

    if std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some()
        && std::env::var_os("SEED_ONLY").is_some()
    {
        lambda_runtime::run(lambda_runtime::service_fn(seed_catalog)).await?;
        return Ok(());
    }

    let in_lambda = std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some();
    if !in_lambda {
        load_env();
    }
    let config = Config::from_env()?;
    let listen = config.listen;
    let runtime = Runtime::connect(config).await?;
    if !in_lambda {
        seed::ensure(&runtime.store)
            .await
            .map_err(|error| error.message().to_string())?;
    }
    let mut catalog = Catalog::load(&runtime.store)
        .await
        .map_err(|error| error.message().to_string())?;
    if !in_lambda {
        catalog::ensure_user(&runtime.store, &mut catalog, &runtime.config.dev_user)
            .await
            .map_err(|error| error.message().to_string())?;
    }
    let app = router(AppState::connect(runtime, catalog));
    if in_lambda {
        lambda_http::run(app).await?;
    } else {
        let listener = tokio::net::TcpListener::bind(listen).await?;
        tracing::info!("local-only api listening on {listen}");
        axum::serve(listener, app).await?;
    }
    Ok(())
}

async fn seed_catalog(
    _event: lambda_runtime::LambdaEvent<serde_json::Value>,
) -> Result<serde_json::Value, lambda_runtime::Error> {
    let config = Config::from_env().map_err(|error| error.to_string())?;
    let runtime = Runtime::connect(config)
        .await
        .map_err(|error| error.to_string())?;
    seed::ensure(&runtime.store)
        .await
        .map_err(|error| error.message().to_string())?;
    Ok(json!({ "ok": true }))
}

fn load_env() {
    let api_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let _ = dotenvy::from_path(api_dir.join("../../.env"));
    let _ = dotenvy::from_path(api_dir.join(".env"));
}

fn router(state: AppState) -> Router {
    let app = Router::new()
        .route("/api/dashboard", get(dashboard))
        .route("/api/lists", get(all_lists).post(create_list))
        .route(
            "/api/lists/{id}",
            get(list_detail).patch(rename_list).delete(delete_list),
        )
        .route("/api/lists/{id}/words", post(add_words))
        .route(
            "/api/lists/{id}/words/{word_id}",
            axum::routing::delete(remove_word),
        )
        .route("/api/queue", put(set_queue))
        .route("/api/words", get(search_words))
        .route("/api/words/drafts", post(drafts))
        .route("/api/words/{id}", get(word_detail))
        .route("/api/session", post(session))
        .route("/api/grades", post(grades))
        .route("/api/speech", post(speak))
        .route("/api/words/{id}/examples", post(generate_example))
        .layer(DefaultBodyLimit::max(1_048_576))
        .route(
            "/api/words/screenshot",
            post(screenshot).layer(DefaultBodyLimit::max(12 * 1024 * 1024)),
        );
    let app = if std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some() {
        app
    } else {
        app.layer(axum::middleware::from_fn(auth::local_boundary))
    };
    app.with_state(state)
}

async fn dashboard(State(state): State<AppState>, User(sub): User) -> ApiResult<impl IntoResponse> {
    let catalog = state.read_catalog().await?;
    Ok(Json(
        dashboard::dashboard(&state.store, &catalog, &sub).await?,
    ))
}

async fn all_lists(State(state): State<AppState>, User(sub): User) -> ApiResult<impl IntoResponse> {
    let catalog = state.read_catalog().await?;
    let studied = study::studied_set(&state.store, &sub).await?;
    Ok(Json(lists::list_summaries(&catalog, &sub, &studied)))
}

#[derive(Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    cursor: usize,
}

async fn list_detail(
    State(state): State<AppState>,
    User(sub): User,
    Path(id): Path<String>,
    Query(query): Query<ListQuery>,
) -> ApiResult<impl IntoResponse> {
    let catalog = state.read_catalog().await?;
    let studied = study::studied_set(&state.store, &sub).await?;
    Ok(Json(lists::list_page(
        &catalog,
        &sub,
        &id,
        &studied,
        query.cursor,
    )?))
}

async fn word_detail(
    State(state): State<AppState>,
    User(sub): User,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let catalog = state.read_catalog().await?;
    Ok(Json(
        dictionary::word_detail(&state.store, &catalog, &sub, &id).await?,
    ))
}

#[derive(Deserialize)]
struct TitleBody {
    title: String,
}

async fn create_list(
    State(state): State<AppState>,
    User(sub): User,
    Json(body): Json<TitleBody>,
) -> ApiResult<impl IntoResponse> {
    let mut catalog = state.catalog.lock().await;
    Ok((
        StatusCode::CREATED,
        Json(lists::create_list(&state.store, &mut catalog, &sub, &body.title).await?),
    ))
}

async fn rename_list(
    State(state): State<AppState>,
    User(sub): User,
    Path(id): Path<String>,
    Json(body): Json<TitleBody>,
) -> ApiResult<impl IntoResponse> {
    let mut catalog = state.catalog.lock().await;
    lists::rename_list(&state.store, &mut catalog, &sub, &id, &body.title).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_list(
    State(state): State<AppState>,
    User(sub): User,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let mut catalog = state.catalog.lock().await;
    lists::delete_list(&state.store, &mut catalog, &sub, &id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct WordsBody {
    words: Vec<WordInput>,
}

async fn add_words(
    State(state): State<AppState>,
    User(sub): User,
    Path(id): Path<String>,
    Json(body): Json<WordsBody>,
) -> ApiResult<impl IntoResponse> {
    let mut catalog = state.catalog.lock().await;
    Ok(Json(
        lists::add_words(&state.store, &mut catalog, &sub, &id, body.words).await?,
    ))
}

async fn remove_word(
    State(state): State<AppState>,
    User(sub): User,
    Path((id, word_id)): Path<(String, String)>,
) -> ApiResult<impl IntoResponse> {
    let mut catalog = state.catalog.lock().await;
    lists::remove_word(&state.store, &mut catalog, &sub, &id, &word_id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct QueueBody {
    ids: Vec<String>,
    version: u64,
}

async fn set_queue(
    State(state): State<AppState>,
    User(sub): User,
    Json(body): Json<QueueBody>,
) -> ApiResult<impl IntoResponse> {
    let mut catalog = state.catalog.lock().await;
    lists::set_queue(&state.store, &mut catalog, &sub, body.ids, body.version).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn search_words(
    State(state): State<AppState>,
    User(sub): User,
    Query(query): Query<SearchQuery>,
) -> ApiResult<impl IntoResponse> {
    let catalog = state.read_catalog().await?;
    Ok(Json(dictionary::search(&catalog, &sub, &query.q)))
}

#[derive(Deserialize)]
struct DraftBody {
    lines: Vec<String>,
}

async fn drafts(
    State(state): State<AppState>,
    User(_): User,
    Json(body): Json<DraftBody>,
) -> ApiResult<impl IntoResponse> {
    let generated = dictionary::fetch_drafts(&state.http, &state.config, body.lines).await?;
    Ok(Json(dictionary::assemble_drafts(generated)?))
}

#[derive(Deserialize)]
struct ScreenshotBody {
    image: String,
}

async fn screenshot(
    State(state): State<AppState>,
    User(sub): User,
    Json(body): Json<ScreenshotBody>,
) -> ApiResult<impl IntoResponse> {
    let extracted = enrichment::extract_screenshot(&state.http, &state.config, &body.image).await?;
    let catalog = state.read_catalog().await?;
    Ok(Json(dictionary::screenshot_preview(
        &catalog, &sub, extracted,
    )?))
}

async fn generate_example(
    State(state): State<AppState>,
    User(sub): User,
    Path(id): Path<String>,
    Json(_body): Json<serde_json::Value>,
) -> ApiResult<impl IntoResponse> {
    let (target, allowed) = {
        let catalog = state.read_catalog().await?;
        dictionary::prepare_example(&catalog, &id)?
    };
    let generated =
        enrichment::generate_example(&state.http, &state.config, &target, &allowed).await?;
    let mut catalog = state.read_catalog().await?;
    dictionary::persist_example(&state.store, &mut catalog, &id, generated).await?;
    Ok(Json(
        dictionary::word_detail(&state.store, &catalog, &sub, &id).await?,
    ))
}

#[derive(Deserialize)]
struct SessionBody {
    unit: String,
    amount: u32,
    direction: Direction,
}

async fn session(
    State(state): State<AppState>,
    User(sub): User,
    Json(body): Json<SessionBody>,
) -> ApiResult<impl IntoResponse> {
    let catalog = state.read_catalog().await?;
    Ok(Json(
        study::session(
            &state.store,
            &catalog,
            &sub,
            &body.unit,
            body.amount,
            body.direction,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GradeBody {
    event_id: String,
    word_id: String,
    direction: String,
    rating: String,
    reviewed_at: i64,
}

async fn grades(
    State(state): State<AppState>,
    User(sub): User,
    Json(body): Json<GradeBody>,
) -> ApiResult<impl IntoResponse> {
    Ok(Json(
        study::apply_grade(
            &state.store,
            &sub,
            &body.event_id,
            &body.word_id,
            &body.direction,
            &body.rating,
            body.reviewed_at,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpeechBody {
    word_id: String,
    sentence_id: Option<String>,
}

async fn speak(
    State(state): State<AppState>,
    User(_): User,
    Json(body): Json<SpeechBody>,
) -> ApiResult<impl IntoResponse> {
    let (text, pinyin) = {
        let catalog = state.read_catalog().await?;
        let word = catalog
            .words
            .get(&body.word_id)
            .ok_or_else(|| crate::error::ApiError::missing("Word not found."))?;
        if let Some(sentence_id) = &body.sentence_id {
            let sentence = word
                .examples
                .iter()
                .find(|sentence| &sentence.id == sentence_id)
                .ok_or_else(|| {
                    crate::error::ApiError::missing("Stored example sentence not found.")
                })?;
            (
                sentence.simplified.clone(),
                sentence.pronounced_pinyin.clone(),
            )
        } else {
            (
                word.simplified.clone(),
                word.pronounced_pinyin
                    .as_ref()
                    .unwrap_or(&word.pinyin)
                    .clone(),
            )
        }
    };
    let identity = body
        .sentence_id
        .as_ref()
        .map(|sentence| format!("{}/{}", body.word_id, sentence))
        .unwrap_or(body.word_id);
    let audio = speech::speak(&state, &text, &pinyin, &identity).await?;
    Ok(([(header::CONTENT_TYPE, "audio/mpeg")], audio))
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[test]
    fn session_requires_an_explicit_direction_and_has_no_production_boolean_alias() {
        let body: SessionBody = serde_json::from_value(json!({
            "unit":"word", "amount":10, "direction":"production"
        }))
        .unwrap();
        assert_eq!(body.direction, Direction::Production);
        assert!(serde_json::from_value::<SessionBody>(json!({
            "unit":"word", "amount":10, "production":true
        }))
        .is_err());
        assert!(serde_json::from_value::<SessionBody>(json!({
            "unit":"word", "amount":10, "direction":"both"
        }))
        .is_err());
    }

    #[test]
    fn speech_accepts_only_word_identity_and_optional_stored_sentence_identity() {
        let word: SpeechBody = serde_json::from_value(json!({"wordId":"w1"})).unwrap();
        assert!(word.sentence_id.is_none());
        let sentence: SpeechBody = serde_json::from_value(json!({
            "wordId":"w1", "sentenceId":"s1"
        }))
        .unwrap();
        assert_eq!(sentence.sentence_id.as_deref(), Some("s1"));
        assert!(serde_json::from_value::<SpeechBody>(json!({
            "text":"untrusted", "pinyin":"untrusted"
        }))
        .is_err());
    }
}
