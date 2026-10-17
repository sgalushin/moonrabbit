use axum::extract::{FromRequestParts, Request};
use axum::http::{header, request::Parts, HeaderMap};
use axum::middleware::Next;
use axum::response::Response;
use lambda_http::RequestExt;

use crate::catalog;
use crate::config::is_loopback_host;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub struct User(pub String);

impl FromRequestParts<AppState> for User {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some() {
            let sub = sub_from_context(parts).ok_or_else(ApiError::unauthorized)?;
            require_sub(&sub)?;
            let mut catalog = state.catalog.lock().await;
            catalog::ensure_user(&state.store, &mut catalog, &sub).await?;
            return Ok(User(sub));
        }
        // This identity is deliberately local-only, not a production authentication fallback.
        Ok(User(state.config.dev_user.clone()))
    }
}

pub fn require_sub(sub: &str) -> ApiResult<()> {
    let ok = (1..=128).contains(&sub.len())
        && sub
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn sub_from_context(parts: &Parts) -> Option<String> {
    let authorizer = parts.request_context_ref()?.authorizer()?;
    authorizer
        .jwt
        .as_ref()?
        .claims
        .get("sub")
        .cloned()
        .filter(|sub| !sub.is_empty())
}

pub async fn local_boundary(request: Request, next: Next) -> ApiResult<Response> {
    validate_headers(request.headers())?;
    Ok(next.run(request).await)
}

fn validate_headers(headers: &HeaderMap) -> ApiResult<()> {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<axum::http::uri::Authority>().ok());
    if !host.is_some_and(|host| is_loopback_host(host.host())) {
        return Err(ApiError::bad("Only loopback Host headers are accepted."));
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let url = origin
            .to_str()
            .ok()
            .and_then(|value| reqwest::Url::parse(value).ok());
        if !url.is_some_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some_and(is_loopback_host)
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
        }) {
            return Err(ApiError::bad("Only loopback browser origins are accepted."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_loopback_proxy_is_allowed_but_remote_origins_are_not() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "127.0.0.1:9860".parse().unwrap());
        headers.insert(header::ORIGIN, "http://localhost:9861".parse().unwrap());
        assert!(validate_headers(&headers).is_ok());
        for origin in ["https://evil.test", "http://localhost.evil.test", "null"] {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            assert!(validate_headers(&headers).is_err());
        }
        headers.remove(header::ORIGIN);
        headers.insert(header::HOST, "evil.test:9850".parse().unwrap());
        assert!(validate_headers(&headers).is_err());
    }

    #[test]
    fn cognito_subjects_are_accepted_and_empty_subjects_are_not() {
        assert!(require_sub("a1b2c3d4-e5f6-7890-abcd-ef1234567890").is_ok());
        assert!(require_sub("local").is_ok());
        assert!(require_sub("").is_err());
        assert!(require_sub("user name").is_err());
    }
}
