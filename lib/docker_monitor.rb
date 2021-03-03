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
    while count < 100
      break unless Docker::Container.get(container.id).json['State']['Running']

      sleep 15
      yield
      count += 1
    end
    raise "Time out exceeded trying to stop container id: #{container.id}"
  rescue Docker::Error::NotFoundError
    true
  end

  def sour_container!(container)
    port = container.json['HostConfig']['PortBindings']['3000/tcp'].first['HostPort']
    body = { key: ENV['KEY'] }

    with_marloss_locker('stop_container') do |locker|
      res = HTTParty.post("http://localhost:#{port}/ping/shutdown", headers: { 'Accept' => 'application/json', 'Content' => 'application/json' }, body: body, verify: false )
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
      container.info['Labels'].each do |label, value|
        containers << container if label == 'com.fenderton.shutdown_over_mem_limit' && value == 'true'
      end
    end
    containers
  end
end
