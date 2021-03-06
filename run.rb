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
    next unless container.mem_usage > container.mem_reservation

    log = { container_id: container.id, task_arn: container.task_arn, event: 'MEMORY_LIMIT_EXCEEDED', duration: container.run_time }
    puts log.to_json

    # send container command to sour the milk
    # on Prise web this will trigger the container to start returning 500 errors in the health check
    container.sour!

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
