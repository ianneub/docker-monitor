# this should use a lot of memory

require 'sinatra'

set :port, 3000
set :bind, '0.0.0.0'

get '/ping' do
  'OK'
end

post '/ping/shutdown' do
  p params
  'SHUTDOWN'
end

nums = []
(1..10_000_000).each do |n|
  nums << n.to_s
end
