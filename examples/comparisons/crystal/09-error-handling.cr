# 09-error-handling.cr
# Error handling patterns.
# Crystal: begin/rescue/ensure for errors. ensure = defer/finally.
# No Result type — exceptions only.

class ConfigError < Exception; end
class FileNotFoundError < ConfigError; end
class ParseError < ConfigError; end

class InvalidValueError < ConfigError
  getter key : String
  getter value : String
  def initialize(@key, @value)
    super("invalid value for #{@key}: #{@value}")
  end
end

class AppError < Exception; end

def read_config(path : String) : String
  file = File.open(path)
  content = file.gets_to_end
  content
rescue ex : File::NotFoundError
  raise FileNotFoundError.new("file not found: #{path}")
ensure
  # ensure is Crystal's defer/finally
  file.try &.close
end

def parse_port(config : String) : Int32
  port = config.to_i? || raise ParseError.new("port must be an integer")

  # Crystal uses || and && for boolean conditions
  if port < 1 || port > 65535
    raise InvalidValueError.new("port", port.to_s)
  end
  port
end

def start_server(port : Int32)
  raise "port must not be zero" if port == 0
  puts "starting server on port #{port}"
end

begin
  config = read_config("app.conf")
  port = parse_port(config)
  start_server(port)
rescue ex : ConfigError
  puts "config error: #{ex.message}"
rescue ex
  puts "unexpected error: #{ex.message}"
end
