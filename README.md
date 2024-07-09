# Docker Monitor

## Notice

This is not the repository you are looking for. This is a WIP prototype. You have been warned.

## Info

This process will watch Docker containers with the following label:

    com.github.ianneub.docker-monitor.shutdown_over_mem_limit=yes

If found it will measure the memory usage of the container and if it exceeds the memory reservation will send an AWS ECS StopTask request to stop the task.
