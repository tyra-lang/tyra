# 05-json-parsing.rb
# JSON parsing with error handling.
# Ruby: JSON.parse returns a Hash. No typed errors — uses exceptions.

require "json"

class JsonError < StandardError; end
class MissingKeyError < JsonError
  attr_reader :key
  def initialize(key)
    @key = key
    super("missing key: #{key}")
  end
end

class TypeMismatchError < JsonError
  attr_reader :expected, :got
  def initialize(expected, got)
    @expected = expected
    @got = got
    super("type error: expected #{expected}, got #{got}")
  end
end

def parse_name(doc)
  raise MissingKeyError.new("name") unless doc.key?("name")
  name = doc["name"]
  raise TypeMismatchError.new("string", name.class.name) unless name.is_a?(String)
  name
end

def load_user_name(input)
  doc = JSON.parse(input)
  parse_name(doc)
rescue JSON::ParserError => e
  raise JsonError, "parse failed: #{e.message}"
end

begin
  name = load_user_name('{"name": "alice"}')
  puts "user: #{name}"
rescue MissingKeyError => e
  puts "missing key: #{e.key}"
rescue TypeMismatchError => e
  puts "type error: expected #{e.expected}, got #{e.got}"
rescue JsonError => e
  puts e.message
end
