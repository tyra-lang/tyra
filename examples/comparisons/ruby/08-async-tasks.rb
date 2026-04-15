# 08-async-tasks.rb
# Concurrent task spawning.
# Ruby: Thread or Fiber for concurrency. Async gem for event-driven IO.
# Using Thread for simplicity (true parallelism with Ractor in Ruby 3+).

require "net/http"
require "uri"

class FetchError < StandardError; end

def fetch(url)
  uri = URI.parse(url)
  response = Net::HTTP.get_response(uri)
  response.body
rescue StandardError => e
  raise FetchError, "network error: #{e.message}"
end

def fetch_all(urls)
  # Spawn threads for concurrent fetching
  threads = urls.map do |url|
    Thread.new { fetch(url) }
  end

  # Join all threads and collect results
  threads.map(&:value)
end

urls = [
  "https://api.example.com/a",
  "https://api.example.com/b",
  "https://api.example.com/c",
]

begin
  results = fetch_all(urls)
  results.each { |item| puts item }
rescue FetchError => e
  puts "error: #{e.message}"
end
