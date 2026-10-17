use std::sync::Arc;

use aws_config::BehaviorVersion;
use aws_sdk_s3::config::{
    Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use tokio::sync::{Mutex, MutexGuard};

use crate::blobs::Blobs;
use crate::catalog::{refresh_catalog, Catalog};
use crate::config::{Config, ConfigResult};
use crate::error::ApiResult;
use crate::speech::SpeechFlights;
use crate::store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub blobs: Blobs,
    pub http: reqwest::Client,
    pub catalog: Arc<Mutex<Catalog>>,
    pub config: Arc<Config>,
    pub speech: SpeechFlights,
}

pub struct Runtime {
    pub store: Store,
    pub config: Arc<Config>,
    blobs: Blobs,
    http: reqwest::Client,
}

impl Runtime {
    pub async fn connect(config: Config) -> ConfigResult<Self> {
        let local = !config.endpoint.is_empty();
        // LocalStack credentials belong to these clients, never to global environment state.
        let shared = if local {
            aws_config::defaults(BehaviorVersion::latest())
                .region(Region::new("us-east-2"))
                .credentials_provider(Credentials::new(
                    "test",
                    "test",
                    None,
                    None,
                    "moonrabbit-local",
                ))
                .endpoint_url(&config.endpoint)
                .load()
                .await
        } else {
            aws_config::defaults(BehaviorVersion::latest()).load().await
        };
        let mut dynamo = aws_sdk_dynamodb::config::Builder::from(&shared);
        let mut s3 = aws_sdk_s3::config::Builder::from(&shared)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired);
        if local {
            dynamo = dynamo.endpoint_url(&config.endpoint);
            s3 = s3.endpoint_url(&config.endpoint).force_path_style(true);
        }
        let dynamo_config = dynamo.build();
        let s3_config = s3.build();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(if local { 60 } else { 25 }))
            .build()?;
        Ok(Self {
            store: Store::new(
                aws_sdk_dynamodb::Client::from_conf(dynamo_config),
                config.table.clone(),
            ),
            blobs: Blobs::new(
                aws_sdk_s3::Client::from_conf(s3_config),
                config.bucket.clone(),
            ),
            http,
            config: Arc::new(config),
        })
    }
}

impl AppState {
    pub fn connect(runtime: Runtime, catalog: Catalog) -> Self {
        Self {
            store: runtime.store,
            blobs: runtime.blobs,
            http: runtime.http,
            catalog: Arc::new(Mutex::new(catalog)),
            config: runtime.config,
            speech: SpeechFlights::default(),
        }
    }

    pub async fn read_catalog(&self) -> ApiResult<MutexGuard<'_, Catalog>> {
        let mut catalog = self.catalog.lock().await;
        refresh_catalog(&self.store, &mut catalog).await?;
        Ok(catalog)
    }
}
