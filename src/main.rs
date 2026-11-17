mod container;
mod lock;
mod log;

use std::time::Duration;

use anyhow::{Context, Result};
use aws_config::meta::region::RegionProviderChain;
use aws_config::{BehaviorVersion, Region};
use bollard::Docker;
use bollard::errors::Error as DockerError;
use serde_json::json;

use crate::container::{Container, UnableToRetrieveStats};
use crate::lock::{LockNotObtained, LockStore};

const CHECK_INTERVAL: Duration = Duration::from_secs(60);
const DOCKER_TIMEOUT: Duration = Duration::from_secs(60);
const LOCK_HASH_KEY: &str = "ID";

#[tokio::main]
async fn main() -> Result<()> {
    let table = std::env::var("DYNAMODB_TABLE")
        .ok()
        .filter(|t| !t.is_empty())
        .context("DynamoDB Table not set (DYNAMODB_TABLE)")?;

    let aws = aws_config::defaults(BehaviorVersion::latest())
        .region(region_provider())
        .load()
        .await;
    let ecs = aws_sdk_ecs::Client::new(&aws);
    let locks = LockStore::new(aws_sdk_dynamodb::Client::new(&aws), table, LOCK_HASH_KEY.to_owned());
    let docker = Docker::connect_with_defaults()
        .context("connecting to Docker")?
        .with_timeout(DOCKER_TIMEOUT);

    println!("Starting to monitor Docker containers...");

    loop {
        check_containers(&docker, &ecs, &locks).await?;
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

/// `AWS_REGION`, then `AWS_DEFAULT_REGION` (which the Rust SDK ignores but the Ruby
/// SDK and the Dockerfile use), then the SDK's default chain.
fn region_provider() -> RegionProviderChain {
    let env_region = |name| std::env::var(name).ok().filter(|r| !r.is_empty()).map(Region::new);
    RegionProviderChain::first_try(env_region("AWS_REGION"))
        .or_else(env_region("AWS_DEFAULT_REGION"))
        .or_default_provider()
}

/// Stops at most one over-limit task per call, then lets the next iteration
/// pick up any others.
async fn check_containers(docker: &Docker, ecs: &aws_sdk_ecs::Client, locks: &LockStore) -> Result<()> {
    for container in find_containers(docker).await? {
        let base = json!({
            "container_id": container.id,
            "task_arn": container.task_arn(),
            "task_definition_family": container.task_definition_family(),
        });

        let usage = match container.mem_usage(docker).await {
            Ok(usage) => usage,
            Err(e) if e.is::<UnableToRetrieveStats>() => {
                log::emit(with(&base, json!({ "event": "COULD_NOT_READ_STATS" })));
                continue;
            }
            Err(e) => return Err(e),
        };
        if usage <= container.mem_reservation(docker).await? {
            continue;
        }

        log::emit(json!({
            "container_id": container.id,
            "task_arn": container.task_arn(),
            "event": "MEMORY_LIMIT_EXCEEDED",
            "task_definition_family": container.task_definition_family(),
            "duration": container.run_time_ms(docker).await?,
        }));

        match container.stop(docker, ecs, locks).await {
            Ok(()) => log::emit(with(&base, json!({ "event": "SHUTDOWN" }))),
            Err(e) if e.is::<LockNotObtained>() => log::emit(with(
                &base,
                json!({
                    "event": "COULD_NOT_OBTAIN_LOCK",
                    "message": e.to_string(),
                    "class": "Marloss::LockNotObtainedError",
                }),
            )),
            Err(e) => return Err(e),
        }
        break;
    }
    Ok(())
}

async fn find_containers(docker: &Docker) -> Result<Vec<Container>> {
    match docker.list_containers(None).await {
        Ok(summaries) => Ok(summaries
            .into_iter()
            .filter_map(Container::from_summary)
            .filter(Container::is_monitored)
            .collect()),
        Err(e @ DockerError::RequestTimeoutError) => {
            log::emit(json!({
                "message": format!("Could not find Docker containers: {e}"),
                "class": "Docker::Error::TimeoutError",
            }));
            Ok(Vec::new())
        }
        Err(e) => Err(e).context("listing Docker containers"),
    }
}

/// Appends `extra`'s keys to a copy of `base`, keeping key order.
fn with(base: &serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let mut merged = base.clone();
    if let (Some(m), serde_json::Value::Object(extra)) = (merged.as_object_mut(), extra) {
        m.extend(extra);
    }
    merged
}
