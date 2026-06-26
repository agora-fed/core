//! # dsoc-storage — sovereign blob storage adapter (ADR-0010 W1.2)
//!
//! Implements [`dsoc_core::Storage`] against any S3-compatible endpoint. The sovereign default is
//! **MinIO** (`http://[::1]:9000`, bucket `dsoc-media`), exposed publicly via Caddy at
//! `/media/*` with `Cache-Control: public, max-age=31536000, immutable` (the gateway always picks
//! a fresh random key on overwrite, so cached URLs stay valid).
//!
//! The adapter exposes ONLY the [`Storage`] port — component crates never see the AWS SDK type
//! surface. A future swap (Cloudflare R2, AWS S3 directly, etc.) is a one-line wiring change
//! in the gateway.
//!
//! Configuration (all required to enable; if any is missing, the adapter is treated as absent):
//! - `STORAGE_ENDPOINT` — base URL of the S3 API (e.g. `http://[::1]:9000`)
//! - `STORAGE_BUCKET`   — bucket name (e.g. `dsoc-media`)
//! - `STORAGE_REGION`   — required by the SDK; arbitrary for MinIO (e.g. `us-east-1`)
//! - `STORAGE_ACCESS_KEY` / `STORAGE_SECRET_KEY` — credentials (PLAN.md principle 8: env only)

#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use dsoc_core::{Error, Result, Storage};

/// Concrete S3-backed [`Storage`].
#[derive(Clone, Debug)]
pub struct S3Storage {
    client: Client,
    bucket: String,
}

impl S3Storage {
    /// Build the adapter explicitly. `endpoint` is the S3 API base URL (without trailing slash).
    /// For MinIO over IPv6 loopback: `http://[::1]:9000`. The SDK still needs a region; for
    /// MinIO pass anything (commonly `us-east-1`).
    ///
    /// Loads the SDK's default async runtime config (sleep impl, HTTP connector) via
    /// `aws_config::defaults`, then layers the explicit endpoint + path-style on top. Without
    /// the defaults the client panics on first send because retry needs an async sleep impl.
    pub async fn new(
        endpoint: impl Into<String>,
        region: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        bucket: impl Into<String>,
    ) -> Self {
        let creds = Credentials::new(
            access_key.into(),
            secret_key.into(),
            None,
            None,
            "dsoc-static",
        );
        let region = Region::new(region.into());
        let sdk_config = aws_config::defaults(BehaviorVersion::latest())
            .region(region.clone())
            .endpoint_url(endpoint.into())
            .credentials_provider(creds)
            .load()
            .await;
        let s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
            // MinIO requires path-style URLs (`<endpoint>/<bucket>/<key>`); the default
            // virtual-host style (`<bucket>.<endpoint>`) doesn't work without DNS.
            .force_path_style(true)
            .build();
        Self {
            client: Client::from_conf(s3_config),
            bucket: bucket.into(),
        }
    }

    /// Try to build from environment. Returns `None` if any required variable is missing,
    /// letting the gateway decide between "fail closed" and "render fallback avatars".
    pub async fn from_env() -> Option<Self> {
        let endpoint = std::env::var("STORAGE_ENDPOINT").ok()?;
        let bucket = std::env::var("STORAGE_BUCKET").ok()?;
        let region = std::env::var("STORAGE_REGION").unwrap_or_else(|_| "us-east-1".to_owned());
        let access_key = std::env::var("STORAGE_ACCESS_KEY").ok()?;
        let secret_key = std::env::var("STORAGE_SECRET_KEY").ok()?;
        Some(Self::new(endpoint, region, access_key, secret_key, bucket).await)
    }

    /// Build and wrap in an `Arc<dyn Storage>` for injection. Convenience for callers that don't
    /// want to import the trait separately.
    #[must_use]
    pub fn into_dyn(self) -> Arc<dyn Storage> {
        Arc::new(self)
    }
}

#[async_trait]
impl Storage for S3Storage {
    async fn put(&self, key: &str, content_type: &str, bytes: Vec<u8>) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| {
                tracing::error!(error = ?e, key, "storage put failed");
                Error::Storage(Box::new(std::io::Error::other(format!(
                    "storage put: {e}"
                ))))
            })?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        // S3 DELETE is idempotent — a missing key returns 204, not an error.
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                tracing::error!(error = ?e, key, "storage delete failed");
                Error::Storage(Box::new(std::io::Error::other(format!(
                    "storage delete: {e}"
                ))))
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn constructor_does_not_panic_on_known_good_input() {
        // Exercises the SDK config wiring (sleep impl, HTTP connector) the runtime path needs.
        let _ = S3Storage::new(
            "http://[::1]:9000",
            "us-east-1",
            "k",
            "s",
            "dsoc-media",
        )
        .await;
    }
}
