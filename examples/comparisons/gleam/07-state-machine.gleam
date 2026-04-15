// 07-state-machine.gleam
// Traffic-light state machine using custom types.
// Gleam has custom types (similar to ADTs) and no mutation.

import gleam/io

pub type Color {
  Red
  Yellow
  Green
}

// Gleam has no value/data distinction; all types are immutable by default.
// There is no copy() — you construct a new record with update syntax.
pub type TrafficLight {
  TrafficLight(color: Color)
}

fn next(light: TrafficLight) -> TrafficLight {
  case light.color {
    Red -> TrafficLight(color: Green)
    Green -> TrafficLight(color: Yellow)
    Yellow -> TrafficLight(color: Red)
  }
}

fn label(color: Color) -> String {
  case color {
    Red -> "stop"
    Yellow -> "caution"
    Green -> "go"
  }
}

fn color_eq(a: Color, b: Color) -> Bool {
  a == b
}

pub fn main() {
  let light = TrafficLight(color: Red)
  let light2 = next(light)
  let light3 = next(light2)
  let light4 = next(light3)

  io.println(label(light.color))
  io.println(label(light2.color))
  io.println(label(light3.color))
  io.println(label(light4.color))
}
