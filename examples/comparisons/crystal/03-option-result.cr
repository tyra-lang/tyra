# 03-option-result.cr
# Nil safety and error handling.
# Crystal uses union types with Nil instead of Option<T>.
# Crystal uses exceptions instead of Result<T, E> — no Result in stdlib.

class LookupError < Exception; end
class NotFoundError < LookupError; end
class InvalidIdError < LookupError; end

def find_user(id : Int32) : String?
  # String? is sugar for String | Nil
  case id
  when 1 then "alice"
  when 2 then "bob"
  else        nil
  end
end

# Crystal: Nil check narrows the type (flow-sensitive typing)
def user_greeting(id : Int32) : String?
  name = find_user(id)
  return nil unless name
  "hello, #{name}"
end

# Crystal: exceptions for recoverable errors (no Result type)
def get_user_result(id : Int32) : String
  raise InvalidIdError.new("invalid id") if id <= 0
  name = find_user(id)
  raise NotFoundError.new("user not found") unless name
  name
end

greeting = user_greeting(1)
if greeting
  puts greeting
else
  puts "not found"
end

begin
  name = get_user_result(0)
  puts "found: #{name}"
rescue NotFoundError
  puts "user not found"
rescue InvalidIdError
  puts "invalid id"
end
