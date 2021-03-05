# frozen_string_literal: true

require 'docker'
require 'httparty'
require 'marloss'

class DockerMonitor
  include Marloss

  marloss_options table: ENV['DYNAMODB_TABLE'], hash_key: 'ID'

  attr_writer :containers

  def initialize
    self.containers = nil
  end

  def containers
    @containers ||= find_containers
  end

  def wait_for_stop(container)
    count = 0
    while true
      break unless Docker::Container.get(container.id).json['State']['Running']

      sleep 15
      yield
      count += 1
      raise "Time out exceeded trying to stop container id: #{container.id}" if count > 100
    end
  rescue Docker::Error::NotFoundError
    true
  end

  def sour_container!(container)
    port = container.json['NetworkSettings']['Ports']['3000/tcp'].first['HostPort']
    body = { key: ENV['KEY'] }

    with_marloss_locker('stop_container') do |locker|
      res = HTTParty.post("http://localhost:#{port}/ping/shutdown", headers: { 'Accept' => 'application/json', 'Content' => 'application/json' }, body: body, verify: false)
      raise 'Could not sour the milk' unless res.code == 200

      wait_for_stop(container) do
        locker.refresh_lock
      end
    end
  end

  protected

  def find_containers
    containers = []
    Docker::Container.all.each do |container|
      containers << container if container.info['Labels'].filter {|label, value| label == 'com.fenderton.shutdown_over_mem_limit' && value == 'yes' }.any?
    end
    containers
  rescue Docker::Error::TimeoutError => e
    log = { message: "Could not find Docker containers: #{e.message}", class: e.class }
    puts logs.to_json
    []
  end
end
