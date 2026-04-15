# 07-state-machine.rb
# Traffic-light state machine.
# Ruby: no ADTs. Uses symbols or classes. Symbols are simplest.

# Ruby has no value/data distinction — everything is a mutable object.
# Frozen objects approximate immutability but it's opt-in, not enforced.

TrafficLight = Struct.new(:color) do
  def next
    new_color = case color
                when :red then :green
                when :green then :yellow
                when :yellow then :red
                end
    TrafficLight.new(new_color)
  end
end

def label(color)
  case color
  when :red then "stop"
  when :yellow then "caution"
  when :green then "go"
  end
end

light = TrafficLight.new(:red)
light2 = light.next
light3 = light2.next
light4 = light3.next

puts label(light.color)
puts label(light2.color)
puts label(light3.color)
puts label(light4.color)
