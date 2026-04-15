# 07-state-machine.cr
# Traffic-light state machine using enum and struct.
# Crystal has enums and structs (value types).

enum Color
  Red
  Yellow
  Green
end

# Crystal: struct is a value type (stack allocated, copied on assignment)
# This is similar to Tyra's `value`
struct TrafficLight
  getter color : Color

  def initialize(@color : Color)
  end

  def next : TrafficLight
    new_color = case color
                when .red?    then Color::Green
                when .green?  then Color::Yellow
                when .yellow? then Color::Red
                else               raise "unreachable"
                end
    TrafficLight.new(new_color)
  end
end

def label(color : Color) : String
  case color
  when .red?    then "stop"
  when .yellow? then "caution"
  when .green?  then "go"
  else               raise "unreachable"
  end
end

light = TrafficLight.new(Color::Red)
light2 = light.next
light3 = light2.next
light4 = light3.next

puts label(light.color)
puts label(light2.color)
puts label(light3.color)
puts label(light4.color)
