# 09-error-handling.rb
# Error handling patterns.
# Ruby: begin/rescue/ensure for errors. ensure is similar to defer.
# No typed Result — exceptions are the standard mechanism.

class ConfigError < StandardError; end
class FileNotFoundError < ConfigError; end
class ParseError < ConfigError; end
class InvalidValueError < ConfigError
  attr_reader :key, :value
  def initialize(key, value)
    @key = key
    @value = value
    super("invalid value for #{key}: #{value}")
  end
end

class AppError < StandardError; end

def read_config(path)
  file = File.open(path)
  content = file.read
  content
rescue Errno::ENOENT
  raise FileNotFoundError, "file not found: #{path}"
ensure
  # ensure is Ruby's defer/finally — always runs
  file&.close
end

def parse_port(config)
  port = Integer(config)
rescue ArgumentError
  raise ParseError, "port must be an integer"
else
  # Ruby: || and && for boolean compound conditions
  if port < 1 || port > 65_535
    raise InvalidValueError.new("port", port.to_s)
  end
  port
end

def start_server(port)
  raise "port must not be zero" if port == 0
  puts "starting server on port #{port}"
end

begin
  config = read_config("app.conf")
  port = parse_port(config)
  start_server(port)
rescue ConfigError => e
  puts "config error: #{e.message}"
rescue => e
  puts "unexpected error: #{e.message}"
end
