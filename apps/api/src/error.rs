use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Clone, Debug)]
pub enum ApiError {
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    Unprocessable(String),
    BadGateway(String),
    Unavailable(String),
    Unauthorized,
}

impl ApiError {
    pub fn bad(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn missing(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }
    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self::Unprocessable(message.into())
    }

    pub fn gateway(message: impl Into<String>) -> Self {
        Self::BadGateway(message.into())
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable(message.into())
    }

    pub fn unauthorized() -> Self {
        Self::Unauthorized
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::BadGateway(_) => StatusCode::BAD_GATEWAY,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::BadRequest(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::Unprocessable(message)
            | Self::BadGateway(message)
            | Self::Unavailable(message) => message,
            Self::Unauthorized => "Sign in is required.",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(json!({ "message": self.message() }));
        (status, body).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

pub fn storage(error: impl std::fmt::Display, message: &str) -> ApiError {
    tracing::error!(error = %error, "{message}");
    ApiError::unavailable(message.to_string())
}
