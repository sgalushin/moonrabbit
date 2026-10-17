use std::env;
use std::net::{IpAddr, SocketAddr};

pub type ConfigResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const AI_MODEL: &str = "anthropic/claude-sonnet-5.5";
const SPEECH_MODEL: &str = "fish-audio/s2.1-pro";

#[derive(Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub dev_user: String,
    pub endpoint: String,
    pub table: String,
    pub bucket: String,
    pub speech_model: String,
    pub ai_model: String,
    pub provider_key: Option<String>,
}

impl Config {
    pub fn from_env() -> ConfigResult<Self> {
        if env::var_os("AWS_LAMBDA_RUNTIME_API").is_some() {
            return deployed();
        }
        if env::var_os("AWS_EXECUTION_ENV")
            .is_some_and(|value| value.to_string_lossy().starts_with("AWS_Lambda"))
        {
            return Err("Moon Rabbit supports local-only operation, not AWS Lambda.".into());
        }
        let mode = value("RUNTIME_MODE", "local")?;
        if mode != "local" {
            return Err(
                "RUNTIME_MODE must be local; deployed/exposed operation is unsupported.".into(),
            );
        }
        let host: IpAddr = value("HOST", "127.0.0.1")?.parse()?;
        if !host.is_loopback() {
            return Err("HOST must be a loopback IP; exposed operation is unsupported.".into());
        }
        let port: u16 = value("PORT", "9850")?.parse()?;
        if port == 0 {
            return Err("PORT must be between 1 and 65535.".into());
        }
        let dev_user = value("DEV_USER", "local")?;
        if dev_user.len() > 128
            || !dev_user
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            return Err(
                "DEV_USER must be 1–128 ASCII letters, digits, dots, underscores, or hyphens."
                    .into(),
            );
        }
        let endpoint = value("LOCALSTACK_ENDPOINT", "http://127.0.0.1:4566")?;
        validate_endpoint(&endpoint)?;
        // Never allow an inherited remote AWS endpoint to silently select a deployed mode.
        for name in [
            "AWS_ENDPOINT_URL",
            "AWS_ENDPOINT_URL_DYNAMODB",
            "AWS_ENDPOINT_URL_S3",
        ] {
            if let Ok(endpoint) = env::var(name) {
                validate_endpoint(&endpoint)?;
            }
        }
        let table = value("TABLE_NAME", "moonrabbit")?;
        let bucket = value("BUCKET_NAME", "moonrabbit-audio")?;
        if !(3..=255).contains(&table.len())
            || !table
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return Err("TABLE_NAME is not a valid local DynamoDB table name.".into());
        }
        if !(3..=63).contains(&bucket.len())
            || !bucket
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '.'))
            || !bucket.as_bytes()[0].is_ascii_alphanumeric()
            || !bucket.as_bytes()[bucket.len() - 1].is_ascii_alphanumeric()
        {
            return Err("BUCKET_NAME is not a valid local S3 bucket name.".into());
        }
        let provider_key = env::var("OPENROUTER_API_KEY")
            .ok()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        Ok(Self {
            listen: SocketAddr::new(host, port),
            dev_user,
            endpoint,
            table,
            bucket,
            speech_model: SPEECH_MODEL.to_string(),
            ai_model: AI_MODEL.to_string(),
            provider_key,
        })
    }
}

fn deployed() -> ConfigResult<Config> {
    let table = value("TABLE_NAME", "")?;
    let bucket = value("BUCKET_NAME", "")?;
    if !(3..=255).contains(&table.len())
        || !table
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err("TABLE_NAME is not a valid DynamoDB table name.".into());
    }
    if !(3..=63).contains(&bucket.len())
        || !bucket
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '.'))
        || !bucket.as_bytes()[0].is_ascii_alphanumeric()
        || !bucket.as_bytes()[bucket.len() - 1].is_ascii_alphanumeric()
    {
        return Err("BUCKET_NAME is not a valid S3 bucket name.".into());
    }
    let provider_key = env::var("OPENROUTER_API_KEY")
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    Ok(Config {
        listen: SocketAddr::from(([127, 0, 0, 1], 9850)),
        dev_user: String::new(),
        endpoint: String::new(),
        table,
        bucket,
        speech_model: SPEECH_MODEL.to_string(),
        ai_model: AI_MODEL.to_string(),
        provider_key,
    })
}

fn value(name: &str, default: &str) -> ConfigResult<String> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => default.to_string(),
        Err(error) => return Err(error.into()),
    };
    if value.is_empty() || value.trim() != value {
        return Err(format!("{name} must be nonempty and have no surrounding whitespace.").into());
    }
    Ok(value)
}

pub fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn validate_endpoint(endpoint: &str) -> ConfigResult<()> {
    let url = reqwest::Url::parse(endpoint)?;
    if url.scheme() != "http"
        || !url.host_str().is_some_and(is_loopback_host)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("AWS storage endpoints must be plain HTTP loopback URLs without credentials, paths, queries, or fragments.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_endpoints_never_leave_loopback() {
        for endpoint in [
            "http://127.0.0.1:4566",
            "http://localhost:4566",
            "http://[::1]:4566",
        ] {
            assert!(validate_endpoint(endpoint).is_ok());
        }
        for endpoint in [
            "http://example.com:4566",
            "http://127.0.0.1.evil.test",
            "http://user@localhost:4566",
            "http://localhost:4566/remote",
            "https://localhost:4566",
        ] {
            assert!(validate_endpoint(endpoint).is_err());
        }
    }
}
