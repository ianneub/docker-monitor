#!/usr/bin/env ruby

# frozen_string_literal: true

require_relative './lib/docker_monitor'

puts 'Starting to monitor Docker containers...'

while (monitor = DockerMonitor.new)
  # check if each container is over its soft memory limit
  monitor.containers.each do |container|
    # soft mem limit
    reservation = container.json['HostConfig']['MemoryReservation']

    stats = container.stats
    mem = stats.dig('memory_stats', 'usage') - stats.dig('memory_stats', 'stats', 'cache')

    next unless mem > reservation

    # send container command to sour the milk
    # on Prise web this will trigger the container to start returning 500 errors in the health check
    monitor.sour_container!(container)

    log = { container_id: container.id, status: 'MEMORY_LIMIT_EXCEEDED' }
    puts log.to_json
  end

  sleep 60
end
