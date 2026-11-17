# Docker Monitor

## Info

This process will watch Docker containers with the following label:

    com.github.ianneub.docker-monitor.shutdown_over_mem_limit=yes

If found it will measure the memory usage of the container and if it exceeds the memory reservation will send an AWS ECS StopTask request to stop the task.

Only one task is stopped at a time across all hosts. This is coordinated through a lock in DynamoDB.

## Configuration

- `DYNAMODB_TABLE` (required): the lock table. It needs a string hash key named `ID`, and TTL should be enabled on the `Expires` attribute. The lock format is compatible with the [marloss](https://github.com/eredi93/marloss) gem used by the earlier Ruby version, so Ruby and Rust instances can share a table.
- AWS region and credentials come from the standard AWS environment variables and the ECS/EC2 credential providers. The region defaults to `us-east-1`.
- The Docker socket (`/var/run/docker.sock`) must be mounted into the container.

## Development

    cargo test
    cargo clippy --all-targets -- -D warnings
    docker compose up --build   # local harness; reads AWS settings from .env
