# 04-http-handler.rb
# HTTP server with multiple route handlers.
# Ruby: Sinatra (lightweight) or Rails. Using Sinatra for comparison.

require "sinatra"

# Ruby has no typed error for handlers — exceptions propagate implicitly.

get "/health" do
  "ok"
end

get "/greet" do
  name = params["name"]
  halt 400, "missing name" unless name
  "hello, #{name}"
end

# Sinatra runs the server when the file is executed.
# No explicit main, no async, no typed return.
