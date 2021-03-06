# frozen_string_literal: true

require 'httparty'
require 'marloss'
require 'aws-sdk-ecs'

class Docker::Container
  include Marloss

  marloss_options table: ENV['DYNAMODB_TABLE'], hash_key: 'ID'

  def mem_reservation
    json['HostConfig']['MemoryReservation']
  end

  def mem_usage
    data = stats
    data.dig('memory_stats', 'usage') - data.dig('memory_stats', 'stats', 'cache')
  end

  def task_arn
    info['Labels'].filter {|label, _| label == 'com.amazonaws.ecs.task-arn' }.first&.last
  end

  def running?
    json['State']['Running']
  end

  def sour!
    port = json['NetworkSettings']['Ports']['3000/tcp'].first['HostPort']
    body = { key: ENV['KEY'] }

    with_marloss_locker('stop_container', retries: 1) do |locker|
      res = HTTParty.post("http://localhost:#{port}/ping/shutdown", headers: { 'Accept' => 'application/json', 'Content' => 'application/json' }, body: body, verify: false)
      raise 'Could not sour the milk' unless res.code == 200

      wait_for_stop(locker)
      wait_for_healthy_ecs_cluster(locker)
    end
  end

  def ecs_cluster
    info['Labels'].filter {|label, _| label == 'com.amazonaws.ecs.cluster' }.first&.last
  end

  def ecs_service
    client = Aws::ECS::Client.new
    res = client.describe_tasks({
      cluster: ecs_cluster,
      tasks: [task_arn]
    })
    res.tasks[0].group.split(':').last
  end

  def healthy_cluster?
    client = Aws::ECS::Client.new
    res = client.describe_services({
      cluster: ecs_cluster,
      services: [ecs_service]
    })
    res.services[0].desired_count == res.services[0].running_count
  end

  protected

  def wait_for_stop(locker)
    count = 0
    loop do
      sleep 15

      refresh!

      break unless running?

      # refresh the lock and increment count. after 100 times, fail.
      locker.refresh_lock
      count += 1
      raise "Time out exceeded trying to stop container id: #{id}" if count > 100
    end
  rescue Docker::Error::NotFoundError
    true
  end

  def wait_for_healthy_ecs_cluster(locker)
    # ensure cluster is healthy before proceeding
    loop do
      locker.refresh_lock
      sleep 15

      break if healthy_cluster?
    end
  end
end
