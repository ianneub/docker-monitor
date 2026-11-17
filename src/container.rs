use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use bollard::Docker;
use bollard::errors::Error as DockerError;
use bollard::models::{ContainerInspectResponse, ContainerSummary};
use bollard::query_parameters::StatsOptionsBuilder;
use futures_util::StreamExt;

use crate::lock::{Lock, LockStore};

pub const MONITOR_LABEL: &str = "com.github.ianneub.docker-monitor.shutdown_over_mem_limit";
const TASK_ARN_LABEL: &str = "com.amazonaws.ecs.task-arn";
const TASK_DEFINITION_FAMILY_LABEL: &str = "com.amazonaws.ecs.task-definition-family";
const CLUSTER_LABEL: &str = "com.amazonaws.ecs.cluster";

const STOP_LOCK_NAME: &str = "stop_container";
const STOP_REASON: &str = "Container over memory limit. Stopping to prevent OOM error.";
const POLL_INTERVAL: Duration = Duration::from_secs(15);
const MAX_STOP_POLLS: u32 = 100;

/// Docker returned stats without `memory_stats.usage` (e.g. the container is exiting).
#[derive(Debug)]
pub struct UnableToRetrieveStats;

impl fmt::Display for UnableToRetrieveStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unable to retrieve memory stats")
    }
}

impl std::error::Error for UnableToRetrieveStats {}

/// A running container, identified by the labels the ECS agent puts on it.
pub struct Container {
    pub id: String,
    labels: HashMap<String, String>,
}

impl Container {
    pub fn from_summary(summary: ContainerSummary) -> Option<Self> {
        Some(Self {
            id: summary.id?,
            labels: summary.labels.unwrap_or_default(),
        })
    }

    pub fn is_monitored(&self) -> bool {
        self.label(MONITOR_LABEL) == Some("yes")
    }

    pub fn task_arn(&self) -> Option<&str> {
        self.label(TASK_ARN_LABEL)
    }

    pub fn task_definition_family(&self) -> Option<&str> {
        self.label(TASK_DEFINITION_FAMILY_LABEL)
    }

    pub fn ecs_cluster(&self) -> Option<&str> {
        self.label(CLUSTER_LABEL)
    }

    fn label(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }

    /// Current memory usage in bytes. Fails with [`UnableToRetrieveStats`] if Docker
    /// doesn't report it.
    pub async fn mem_usage(&self, docker: &Docker) -> Result<u64> {
        let options = StatsOptionsBuilder::new().stream(false).build();
        let stats = docker
            .stats(&self.id, Some(options))
            .next()
            .await
            .ok_or(UnableToRetrieveStats)??;
        stats
            .memory_stats
            .and_then(|m| m.usage)
            .ok_or_else(|| UnableToRetrieveStats.into())
    }

    /// The soft memory limit (`HostConfig.MemoryReservation`) in bytes; 0 when unset.
    pub async fn mem_reservation(&self, docker: &Docker) -> Result<u64> {
        let reservation = self
            .inspect(docker)
            .await?
            .host_config
            .and_then(|h| h.memory_reservation)
            .unwrap_or(0);
        Ok(reservation.max(0) as u64)
    }

    /// Milliseconds since the container started.
    pub async fn run_time_ms(&self, docker: &Docker) -> Result<f64> {
        let started_at = self
            .inspect(docker)
            .await?
            .state
            .and_then(|s| s.started_at)
            .ok_or_else(|| anyhow!("container {} has no StartedAt", self.id))?;
        let started_at = chrono::DateTime::parse_from_rfc3339(&started_at)
            .with_context(|| format!("parsing StartedAt {started_at:?}"))?;
        let elapsed = chrono::Utc::now().signed_duration_since(started_at);
        Ok(elapsed.num_microseconds().unwrap_or(i64::MAX) as f64 / 1_000.0)
    }

    async fn inspect(&self, docker: &Docker) -> Result<ContainerInspectResponse> {
        docker
            .inspect_container(&self.id, None)
            .await
            .with_context(|| format!("inspecting container {}", self.id))
    }

    /// Stops the container's ECS task, then waits for the container to exit and the
    /// service to return to its desired count. Holds the cluster-wide stop lock
    /// throughout, so only one task is stopped at a time across all hosts.
    pub async fn stop(&self, docker: &Docker, ecs: &aws_sdk_ecs::Client, locks: &LockStore) -> Result<()> {
        locks
            .with_lock(STOP_LOCK_NAME, Some(1), async |lock| {
                ecs.stop_task()
                    .set_cluster(self.ecs_cluster().map(str::to_owned))
                    .set_task(self.task_arn().map(str::to_owned))
                    .reason(STOP_REASON)
                    .send()
                    .await
                    .context("ECS StopTask")?;

                self.wait_for_stop(docker, lock).await?;
                self.wait_for_healthy_ecs_service(ecs, lock).await
            })
            .await
    }

    async fn wait_for_stop(&self, docker: &Docker, lock: &Lock<'_>) -> Result<()> {
        let mut count = 0;
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;

            let running = match docker.inspect_container(&self.id, None).await {
                Ok(info) => info.state.and_then(|s| s.running).unwrap_or(false),
                Err(DockerError::DockerResponseServerError { status_code: 404, .. }) => return Ok(()),
                Err(e) => return Err(e).with_context(|| format!("inspecting container {}", self.id)),
            };
            if !running {
                return Ok(());
            }

            lock.refresh().await?;
            count += 1;
            if count > MAX_STOP_POLLS {
                bail!("Time out exceeded trying to stop container id: {}", self.id);
            }
        }
    }

    async fn wait_for_healthy_ecs_service(&self, ecs: &aws_sdk_ecs::Client, lock: &Lock<'_>) -> Result<()> {
        loop {
            lock.refresh().await?;
            tokio::time::sleep(POLL_INTERVAL).await;

            if self.ecs_service_healthy(ecs).await? {
                return Ok(());
            }
        }
    }

    async fn ecs_service_name(&self, ecs: &aws_sdk_ecs::Client) -> Result<String> {
        let res = ecs
            .describe_tasks()
            .set_cluster(self.ecs_cluster().map(str::to_owned))
            .set_tasks(self.task_arn().map(|arn| vec![arn.to_owned()]))
            .send()
            .await
            .context("ECS DescribeTasks")?;
        let group = res
            .tasks()
            .first()
            .and_then(|t| t.group())
            .ok_or_else(|| anyhow!("ECS task {:?} not found or has no group", self.task_arn()))?;
        Ok(service_name_from_group(group).to_owned())
    }

    async fn ecs_service_healthy(&self, ecs: &aws_sdk_ecs::Client) -> Result<bool> {
        let service = self.ecs_service_name(ecs).await?;
        let res = ecs
            .describe_services()
            .set_cluster(self.ecs_cluster().map(str::to_owned))
            .services(&service)
            .send()
            .await
            .context("ECS DescribeServices")?;
        let svc = res
            .services()
            .first()
            .ok_or_else(|| anyhow!("ECS service {service} not found"))?;
        Ok(svc.desired_count() == svc.running_count())
    }
}

/// An ECS task's `group` is `service:<name>` for service tasks.
fn service_name_from_group(group: &str) -> &str {
    group.rsplit(':').next().unwrap_or(group)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(labels: &[(&str, &str)]) -> Container {
        Container {
            id: "abc".into(),
            labels: labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn monitored_only_when_label_is_yes() {
        assert!(container(&[(MONITOR_LABEL, "yes")]).is_monitored());
        assert!(!container(&[(MONITOR_LABEL, "no")]).is_monitored());
        assert!(!container(&[(MONITOR_LABEL, "YES")]).is_monitored());
        assert!(!container(&[]).is_monitored());
    }

    #[test]
    fn reads_ecs_labels() {
        let c = container(&[
            (TASK_ARN_LABEL, "arn:aws:ecs:us-east-1:1:task/prod/abc"),
            (CLUSTER_LABEL, "prod"),
            (TASK_DEFINITION_FAMILY_LABEL, "worker"),
        ]);
        assert_eq!(c.task_arn(), Some("arn:aws:ecs:us-east-1:1:task/prod/abc"));
        assert_eq!(c.ecs_cluster(), Some("prod"));
        assert_eq!(c.task_definition_family(), Some("worker"));
        assert_eq!(container(&[]).task_arn(), None);
    }

    #[test]
    fn service_name_is_last_group_segment() {
        assert_eq!(service_name_from_group("service:web"), "web");
        assert_eq!(service_name_from_group("family:worker"), "worker");
        assert_eq!(service_name_from_group("plain"), "plain");
    }
}
