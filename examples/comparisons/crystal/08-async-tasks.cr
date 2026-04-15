# 08-async-tasks.cr
# Concurrent task spawning.
# Crystal uses Fibers (lightweight green threads) and Channels.
# Similar to Go's goroutines and channels.

require "http/client"

class FetchError < Exception; end

def fetch(url : String) : String
  response = HTTP::Client.get(url)
  response.body
rescue ex
  raise FetchError.new("network error: #{ex.message}")
end

def fetch_all(urls : Array(String)) : Array(String)
  channel = Channel(String | FetchError).new

  urls.each do |url|
    spawn do
      begin
        channel.send(fetch(url))
      rescue ex : FetchError
        channel.send(ex)
      end
    end
  end

  # Collect results from all fibers
  results = Array(String).new
  urls.size.times do
    result = channel.receive
    case result
    when String
      results << result
    when FetchError
      raise result
    end
  end
  results
end

urls = [
  "https://api.example.com/a",
  "https://api.example.com/b",
  "https://api.example.com/c",
]

begin
  results = fetch_all(urls)
  results.each { |item| puts item }
rescue ex : FetchError
  puts "error: #{ex.message}"
end
