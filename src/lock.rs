//! Distributed lock in DynamoDB, wire-compatible with the Ruby `marloss` gem
//! (v0.5) so Rust and Ruby instances can share one lock table during a rollout.
//!
//! Item layout: `{<hash_key>: name (S), ProcessID: "hostname:pid" (S), Expires: unix secs (N)}`.
//! A lock can be taken if it doesn't exist, has expired, or is already held by this process.

use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::ProvideErrorMetadata;
use aws_sdk_dynamodb::types::AttributeValue;
use serde_json::json;

use crate::log;

const EXPIRES_KEY: &str = "Expires";
const PROCESS_ID_KEY: &str = "ProcessID";
const TTL_SECS: i64 = 30;
const RETRY_SLEEP: Duration = Duration::from_secs(3);

/// Another process holds the lock. Mirrors `Marloss::LockNotObtainedError`.
#[derive(Debug)]
pub struct LockNotObtained(pub String);

impl fmt::Display for LockNotObtained {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LockNotObtained {}

/// Refreshing a lock we thought we held failed. Mirrors `Marloss::LockNotRefreshedError`.
#[derive(Debug)]
pub struct LockNotRefreshed(pub String);

impl fmt::Display for LockNotRefreshed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LockNotRefreshed {}

pub struct LockStore {
    client: Client,
    table: String,
    hash_key: String,
    process_id: String,
}

/// A held lock. Pass it to long-running work so it can call [`Lock::refresh`].
pub struct Lock<'a> {
    store: &'a LockStore,
    name: &'a str,
}

impl Lock<'_> {
    pub async fn refresh(&self) -> Result<()> {
        self.store.refresh_lock(self.name).await
    }
}

impl LockStore {
    pub fn new(client: Client, table: String, hash_key: String) -> Self {
        let hostname = gethostname::gethostname().to_string_lossy().into_owned();
        let process_id = format!("{hostname}:{}", std::process::id());
        Self {
            client,
            table,
            hash_key,
            process_id,
        }
    }

    /// Takes the lock, runs `body`, and always releases the lock afterwards.
    ///
    /// `retries` matches marloss: `None` retries forever; `Some(n)` gives up with
    /// [`LockNotObtained`] after `n` failed attempts (each followed by a 3s sleep).
    pub async fn with_lock<T>(
        &self,
        name: &str,
        retries: Option<u32>,
        body: impl AsyncFnOnce(&Lock<'_>) -> Result<T>,
    ) -> Result<T> {
        self.wait_until_lock_obtained(name, retries).await?;
        let lock = Lock { store: self, name };
        let result = body(&lock).await;
        let released = self.delete_lock(name).await;
        let value = result?;
        released?;
        Ok(value)
    }

    async fn wait_until_lock_obtained(&self, name: &str, mut retries: Option<u32>) -> Result<()> {
        loop {
            match self.create_lock(name).await {
                Err(e) if e.is::<LockNotObtained>() => {
                    tokio::time::sleep(RETRY_SLEEP).await;
                    if let Some(n) = retries.as_mut() {
                        *n = n.saturating_sub(1);
                        if *n == 0 {
                            return Err(e);
                        }
                    }
                }
                other => return other,
            }
        }
    }

    async fn create_lock(&self, name: &str) -> Result<()> {
        let now = unix_now();
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .item(&self.hash_key, AttributeValue::S(name.to_owned()))
            .item(PROCESS_ID_KEY, AttributeValue::S(self.process_id.clone()))
            .item(EXPIRES_KEY, AttributeValue::N((now + TTL_SECS).to_string()))
            .expression_attribute_names("#E", EXPIRES_KEY)
            .expression_attribute_names("#P", PROCESS_ID_KEY)
            .expression_attribute_values(":now", AttributeValue::N(now.to_string()))
            .expression_attribute_values(":process_id", AttributeValue::S(self.process_id.clone()))
            .condition_expression(format!(
                "attribute_not_exists({}) OR #E < :now OR #P = :process_id",
                self.hash_key
            ))
            .send()
            .await;

        match result {
            Ok(_) => {
                info(format!(
                    "Lock for {name} created successfully, will expire in {TTL_SECS} seconds"
                ));
                Ok(())
            }
            Err(e)
                if e.as_service_error()
                    .is_some_and(|se| se.is_conditional_check_failed_exception()) =>
            {
                error(format!("Failed to create lock for {name}"));
                Err(LockNotObtained(error_message(&e)).into())
            }
            Err(e) => Err(e).with_context(|| format!("creating lock {name}")),
        }
    }

    async fn refresh_lock(&self, name: &str) -> Result<()> {
        let now = unix_now();
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key(&self.hash_key, AttributeValue::S(name.to_owned()))
            .expression_attribute_names("#E", EXPIRES_KEY)
            .expression_attribute_names("#P", PROCESS_ID_KEY)
            .expression_attribute_values(":expires", AttributeValue::N((now + TTL_SECS).to_string()))
            .expression_attribute_values(":now", AttributeValue::N(now.to_string()))
            .expression_attribute_values(":process_id", AttributeValue::S(self.process_id.clone()))
            .update_expression("SET #E = :expires")
            .condition_expression(format!(
                "attribute_exists({}) AND (#E < :now OR #P = :process_id)",
                self.hash_key
            ))
            .send()
            .await;

        match result {
            Ok(_) => {
                info(format!(
                    "Lock for {name} refreshed successfully, will expire in {TTL_SECS} seconds"
                ));
                Ok(())
            }
            Err(e)
                if e.as_service_error()
                    .is_some_and(|se| se.is_conditional_check_failed_exception()) =>
            {
                error(format!("Failed to refresh lock for {name}"));
                Err(LockNotRefreshed(error_message(&e)).into())
            }
            Err(e) => Err(e).with_context(|| format!("refreshing lock {name}")),
        }
    }

    async fn delete_lock(&self, name: &str) -> Result<()> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key(&self.hash_key, AttributeValue::S(name.to_owned()))
            .send()
            .await
            .with_context(|| format!("deleting lock {name}"))?;
        info(format!("Lock for {name} deleted successfully"));
        Ok(())
    }
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn error_message<E: ProvideErrorMetadata>(e: &E) -> String {
    e.message().unwrap_or("The conditional request failed").to_owned()
}

fn info(message: String) {
    log::emit(json!({ "level": "INFO", "message": message }));
}

fn error(message: String) {
    log::emit(json!({ "level": "ERROR", "message": message }));
}
