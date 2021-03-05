require 'httparty'
require 'marloss'

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

  def sour!
    port = json['NetworkSettings']['Ports']['3000/tcp'].first['HostPort']
    body = { key: ENV['KEY'] }

    with_marloss_locker('stop_container', retries: 1) do |locker|
      res = HTTParty.post("http://localhost:#{port}/ping/shutdown", headers: { 'Accept' => 'application/json', 'Content' => 'application/json' }, body: body, verify: false)
      raise 'Could not sour the milk' unless res.code == 200

      wait_for_stop do
        locker.refresh_lock
      end
    end
  end

  protected

  def wait_for_stop
    count = 0
    while true
      refresh!
      break unless json['State']['Running']

      sleep 15
      yield
      count += 1
      raise "Time out exceeded trying to stop container id: #{id}" if count > 100
    end
  rescue Docker::Error::NotFoundError
    true
  end
end
