# this should use a lot of memory

nums = []
(1..10_000_000).each do |n|
  nums << n.to_s
end

loop do
  sleep 5
end
