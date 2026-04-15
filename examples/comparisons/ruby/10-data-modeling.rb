# 10-data-modeling.rb
# Data modeling with classes and structs.
# Ruby: no value/data distinction. Everything is a mutable object.
# No static types, no ability derivation, no compile-time constraints.

# Struct for simple value-like types (still mutable in Ruby)
Point = Struct.new(:x, :y) do
  def to_s
    "(#{x}, #{y})"
  end
end

UserId = Struct.new(:id) do
  include Comparable
  def <=>(other)
    id <=> other.id
  end
end

class User
  attr_accessor :name, :email
  attr_reader :id

  def initialize(id:, name:, email:)
    @id = id
    @name = name
    @email = email
  end

  def to_s
    "User(#{id.id}, #{name})"
  end
end

def distance_squared(a, b)
  dx = a.x - b.x
  dy = a.y - b.y
  dx * dx + dy * dy
end

# Ruby: mutation is the default. No mut/let distinction.
def rename(user, new_name)
  user.name = new_name
end

origin = Point.new(0.0, 0.0)
p = Point.new(3.0, 4.0)
p2 = p.dup.tap { |pt| pt.x = 1.0 }  # Ruby: dup + tap for "copy with update"

puts origin
puts p
puts p2

dist_sq = distance_squared(origin, p)
puts "distance squared: #{dist_sq}"

# Ruby: == on Struct compares all fields (including Float!)
# No compile-time warning for Float comparison.
if origin == p
  puts "same point"
else
  puts "different points"
end

user = User.new(id: UserId.new(1), name: "alice", email: "alice@example.com")
rename(user, "alice smith")
puts user

id1 = UserId.new(1)
id2 = UserId.new(2)
if id1 < id2
  puts "id1 is smaller"
else
  puts "id2 is smaller or equal"
end
