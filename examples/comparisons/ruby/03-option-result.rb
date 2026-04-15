# 03-option-result.rb
# Option and Result usage patterns.
# Ruby has no Option/Result in stdlib. Uses nil for absence and exceptions for errors.
# dry-monads gem provides Some/None/Success/Failure, but idiomatic Ruby uses nil + raise.

# Idiomatic Ruby: nil for absence, raise for errors.

class LookupError < StandardError; end
class NotFoundError < LookupError; end
class InvalidIdError < LookupError; end

def find_user(id)
  case id
  when 1 then "alice"
  when 2 then "bob"
  else nil
  end
end

# Ruby: nil check replaces Option pattern
def user_greeting(id)
  name = find_user(id)
  return nil unless name
  "hello, #{name}"
end

# Ruby: exceptions replace Result pattern
def get_user_result(id)
  raise InvalidIdError, "invalid id" if id <= 0
  name = find_user(id)
  raise NotFoundError, "user not found" unless name
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
