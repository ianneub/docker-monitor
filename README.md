# Docker Monitor

This process will watch Docker containers with the following label:

    com.fenderton.shutdown_over_mem_limit=true

If found it will measure the memory usage of the container and if it exceeds the memory reservation will send an http POST request to the container that signals the container to start returning failed health checks. Thus allowing the container to gracefully shutdown.

This process will not be needed once ECS implements this:
https://github.com/aws/containers-roadmap/issues/576
