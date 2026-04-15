# 04-http-handler.cr
# HTTP server with multiple route handlers.
# Crystal has HTTP::Server in stdlib. Kemal is the popular framework.
# Using stdlib HTTP::Server for a fair comparison.

require "http/server"

server = HTTP::Server.new do |context|
  path = context.request.path

  case path
  when "/health"
    context.response.status_code = 200
    context.response.print "ok"
  when "/greet"
    name = context.request.query_params["name"]?
    if name
      context.response.status_code = 200
      context.response.print "hello, #{name}"
    else
      context.response.status_code = 400
      context.response.print "missing name"
    end
  else
    context.response.status_code = 404
    context.response.print "not found"
  end
end

# Crystal: server.listen is blocking (uses Fibers internally)
puts "listening on port 8080"
server.listen("0.0.0.0", 8080)
