#!/usr/bin/env ruby

# frozen_string_literal: true

$stdout.sync = true

require_relative './lib/docker_monitor'

puts 'Starting to monitor Docker containers...'

loop do
  monitor = DockerMonitor.new
  # check if each container is over its soft memory limit
  monitor.containers.each do |container|
    # skip this container unless mem_usage exceeds mem_reservation
    begin
      next unless container.mem_usage > container.mem_reservation
    rescue Docker::Container::UnableToRetrieveStats
      log = { container_id: container.id, task_arn: container.task_arn, event: 'COULD_NOT_READ_STATS' }
      puts log.to_json

      next
    end

    log = { container_id: container.id, task_arn: container.task_arn, event: 'MEMORY_LIMIT_EXCEEDED', duration: container.run_time * 1_000 }
    puts log.to_json

    # send container command to stop the container
    container.stop!

    log = { container_id: container.id, task_arn: container.task_arn, event: 'SHUTDOWN' }
    puts log.to_json

    # stop processing containers. wait until the next iteration to continue.
    break
  rescue Marloss::LockNotObtainedError => e
    log = { container_id: container.id, task_arn: container.task_arn, event: 'COULD_NOT_OBTAIN_LOCK', message: e.message, class: e.class }
    puts log.to_json
    break
  end

  sleep 60
end
