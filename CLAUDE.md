# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A long-running Rust process, used in production, that runs on an ECS container host and stops ECS tasks whose containers use more memory than their soft limit (`MemoryReservation`). It only watches containers labeled `com.github.ianneub.docker-monitor.shutdown_over_mem_limit=yes`. It was ported from Ruby; the Ruby version is in git history before the `rust-rewrite` branch.

## Commands

- Test: `cargo test` (single test: `cargo test service_name_is_last_group_segment`)
- Lint and format: `cargo clippy --all-targets -- -D warnings` and `cargo fmt` (120-column width, see `rustfmt.toml`)
- Run: `DYNAMODB_TABLE=... cargo run` (needs AWS credentials and the Docker socket)
- Local harness: `docker compose up --build`. It reads env from `.env` (gitignored) and starts two fixture containers from `test/`: `good.rb` idles under a 128M reservation, and `bad.rb` uses about 500MB against an 8M reservation. The fixtures are plain Ruby scripts run on the stock `ruby` image; they are workloads, not tests. Their ECS labels are fake, so the `StopTask` call fails unless you point `AWS_ENDPOINT_URL_ECS` at a mock.

Rust isn't necessarily installed on the host. If `cargo` is missing, build in the `rust:1.100` Docker image.

The Rust version is pinned in two places, `.tool-versions` and the Dockerfile's `rust:1.100-alpine`; change both together. The `Dockerfile` cross-compiles with `tonistiigi/xx` (pinned) on the build platform instead of compiling under QEMU. It targets musl to get a fully static binary, and the final image is `scratch`: just the binary and a CA bundle, which the AWS SDK needs for TLS. The image has no shell, so `docker exec ... sh` won't work. Any new dependency must build statically for musl; `xx-verify --static` fails the build otherwise. CI (`.github/workflows/build.yml`) only builds and pushes a multi-arch (amd64/arm64) image to `ghcr.io` on pushes to `master`. It runs no tests or lint.

## Architecture

- `src/main.rs`: config and the main loop. Every 60s it checks each labeled container and stops **at most one** container per pass (`break` after the first stop or after failing to get the lock). Errors not handled explicitly propagate out of `main`, so the process exits and gets restarted; this matches the Ruby version on purpose.
- `src/container.rs`: Docker reads (stats, inspect) through `bollard`, the ECS label readers, and the stop flow.
- `src/lock.rs`: a DynamoDB lock that reproduces the Ruby `marloss` gem (v0.5) **exactly**, so Ruby and Rust instances can share a lock table during a rollout. Item: `{ID: name (S), ProcessID: "hostname:pid" (S), Expires: unix secs (N)}`, 30s TTL, with the same condition expressions. Don't change the item layout or the conditions without considering instances that still run the old version.

### Stop flow

`Container::stop` takes the global `stop_container` lock (`retries: Some(1)` means one attempt, then a 3s sleep, then it gives up with `LockNotObtained`). It holds the lock across all hosts while it:
1. Calls ECS `StopTask` (not a Docker stop) so ECS replaces the task.
2. Polls Docker every 15s until the container is no longer running or is gone, refreshing the lock each time. It gives up after 100 polls.
3. Polls ECS until the task's service (from the task's `group`, `service:<name>`) has `desired_count == running_count`, refreshing the lock each time.

The lock is always released afterwards, even on error.

### Output contract

stdout is one JSON object per line (`src/log.rs`). The keys stay in the order they were written because `serde_json` is built with `preserve_order`. Log queries may depend on the `event` values (`MEMORY_LIMIT_EXCEEDED`, `SHUTDOWN`, `COULD_NOT_READ_STATS`, `COULD_NOT_OBTAIN_LOCK`), the key order, and the Ruby-era `class` strings (`Marloss::LockNotObtainedError`, `Docker::Error::TimeoutError`). Keep all of these stable.

### AWS config

The region comes from `AWS_REGION`, then `AWS_DEFAULT_REGION` (the Rust SDK doesn't read that variable itself, so `main.rs` does), then the SDK default chain. The Dockerfile sets `AWS_DEFAULT_REGION=us-east-1`. `AWS_ENDPOINT_URL_ECS` and `AWS_ENDPOINT_URL_DYNAMODB` work for pointing at local mocks.
