# 10-data-modeling.cr
# Data modeling with struct (value type) and class (reference type).
# Crystal has both struct and class — closest analog to Tyra's value/data.

# Crystal: struct is a value type (stack, copied on assignment)
# Similar to Tyra's `value`
struct Point
  getter x : Float64
  getter y : Float64

  def initialize(@x, @y)
  end

  def to_s : String
    "(#{x}, #{y})"
  end

  # Crystal: copy() equivalent — create new struct with modified fields
  def copy(x : Float64 = @x, y : Float64 = @y) : Point
    Point.new(x, y)
  end
end

struct UserId
  getter id : Int32
  include Comparable(UserId)

  def initialize(@id)
  end

  def <=>(other : UserId) : Int32
    id <=> other.id
  end
end

# Crystal: class is a reference type (heap, GC managed)
# Similar to Tyra's `data`
class User
  getter id : UserId
  property name : String     # property = getter + setter (like Tyra's mut field)
  property email : String

  def initialize(@id, @name, @email)
  end

  def to_s : String
    "User(#{id.id}, #{name})"
  end
end

def distance_squared(a : Point, b : Point) : Float64
  dx = a.x - b.x
  dy = a.y - b.y
  dx * dx + dy * dy
end

# Crystal: class is reference type, so mutation affects caller
def rename(user : User, new_name : String)
  user.name = new_name
end

origin = Point.new(0.0, 0.0)
p = Point.new(3.0, 4.0)
p2 = p.copy(x: 1.0)

puts origin
puts p
puts p2

dist_sq = distance_squared(origin, p)
puts "distance squared: #{dist_sq}"

# Crystal: == on structs compares all fields (including Float!)
# No compile-time protection against Float comparison.
if origin == p
  puts "same point"
else
  puts "different points"
end

user = User.new(UserId.new(1), "alice", "alice@example.com")
rename(user, "alice smith")
puts user

id1 = UserId.new(1)
id2 = UserId.new(2)
if id1 < id2
  puts "id1 is smaller"
else
  puts "id2 is smaller or equal"
end
