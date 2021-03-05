# frozen_string_literal: true

require 'docker'
require_relative './docker/container'

class DockerMonitor
  attr_writer :containers

  def initialize
    self.containers = nil
  end

  def containers
    @containers ||= find_containers
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
    puts log.to_json
    []
  end
end
