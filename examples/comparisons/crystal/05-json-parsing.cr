# 05-json-parsing.cr
# JSON parsing with error handling.
# Crystal has JSON.parse (returns JSON::Any) and JSON::Serializable (struct mapping).
# Error handling uses exceptions.

require "json"

class JsonError < Exception; end
class MissingKeyError < JsonError
  getter key : String
  def initialize(@key)
    super("missing key: #{@key}")
  end
end

class TypeMismatchError < JsonError
  getter expected : String
  getter got : String
  def initialize(@expected, @got)
    super("type error: expected #{@expected}, got #{@got}")
  end
end

class AppError < Exception; end
class JsonAppError < AppError
  getter inner : JsonError
  def initialize(@inner)
    super(@inner.message)
  end
end

class IoAppError < AppError
  def initialize(message : String)
    super(message)
  end
end

def parse_name(doc : JSON::Any) : String
  name_val = doc["name"]? || raise MissingKeyError.new("name")
  name_val.as_s? || raise TypeMismatchError.new("string", name_val.raw.class.name)
end

def load_user_name(input : String) : String
  doc = JSON.parse(input)
  parse_name(doc)
rescue ex : JSON::ParseException
  raise JsonAppError.new(JsonError.new("parse failed: #{ex.message}"))
rescue ex : JsonError
  raise JsonAppError.new(ex)
end

begin
  name = load_user_name(%q({"name": "alice"}))
  puts "user: #{name}"
rescue ex : JsonAppError
  case inner = ex.inner
  when MissingKeyError
    puts "missing key: #{inner.key}"
  when TypeMismatchError
    puts "type error: expected #{inner.expected}, got #{inner.got}"
  else
    puts "parse failed: #{ex.message}"
  end
rescue ex : IoAppError
  puts "io error: #{ex.message}"
end
