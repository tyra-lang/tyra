// 07-state-machine.v
// Traffic-light state machine using enums and structs.

enum Color {
	red
	yellow
	green
}

struct TrafficLight {
	color Color
}

fn next(light TrafficLight) TrafficLight {
	new_color := match light.color {
		.red { Color.green }
		.green { Color.yellow }
		.yellow { Color.red }
	}
	return TrafficLight{
		color: new_color
	}
}

fn label(color Color) string {
	return match color {
		.red { 'stop' }
		.yellow { 'caution' }
		.green { 'go' }
	}
}

fn main() {
	light := TrafficLight{color: .red}
	light2 := next(light)
	light3 := next(light2)
	light4 := next(light3)

	println(label(light.color))
	println(label(light2.color))
	println(label(light3.color))
	println(label(light4.color))
}
