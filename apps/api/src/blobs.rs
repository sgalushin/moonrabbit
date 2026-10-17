use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use axum::body::Bytes;

use crate::error::{storage, ApiResult};

#[derive(Clone)]
pub struct Blobs {
    client: Client,
    bucket: String,
}

impl Blobs {
    pub fn new(client: Client, bucket: String) -> Self {
        Self { client, bucket }
    }

    pub async fn put_bytes(&self, key: &str, body: Bytes, content_type: &str) -> ApiResult<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(body))
            .content_type(content_type)
            .send()
            .await
            .map_err(|error| storage(error, "The clip could not be stored."))?;
        Ok(())
    }

    pub async fn get_bytes(&self, key: &str) -> ApiResult<Option<Bytes>> {
        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(output) => {
                let bytes = output
                    .body
                    .collect()
                    .await
                    .map_err(|error| storage(error, "The clip could not be read."))?
                    .into_bytes();
                Ok(Some(bytes))
            }
            Err(error) => {
                let missing = error
                    .as_service_error()
                    .map(|service| service.is_no_such_key())
                    .unwrap_or(false);
                if missing {
                    Ok(None)
                } else {
                    Err(storage(error, "The clip could not be read."))
                }
            }
        }
    }
}
