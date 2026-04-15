// 10-data-modeling.gleam
// Data modeling with custom types and traits.
// Gleam: all data is immutable. No mut fields. No value/data distinction.
// No trait system — use module functions instead.

import gleam/float
import gleam/int
import gleam/io
import gleam/order

pub type Point {
  Point(x: Float, y: Float)
}

pub type UserId {
  UserId(id: Int)
}

pub type User {
  User(id: UserId, name: String, email: String)
}

// Gleam has no traits. "Stringable" is just a function per type.
fn point_to_string(p: Point) -> String {
  "(" <> float.to_string(p.x) <> ", " <> float.to_string(p.y) <> ")"
}

fn user_to_string(u: User) -> String {
  "User(" <> int.to_string(u.id.id) <> ", " <> u.name <> ")"
}

fn distance_squared(a: Point, b: Point) -> Float {
  let dx = a.x -. b.x
  let dy = a.y -. b.y
  dx *. dx +. dy *. dy
}

// Gleam: all data is immutable. "Rename" returns a new User with updated name.
// Uses record update syntax.
fn rename(user: User, new_name: String) -> User {
  User(..user, name: new_name)
}

pub fn main() {
  let origin = Point(x: 0.0, y: 0.0)
  let p = Point(x: 3.0, y: 4.0)
  let p2 = Point(..p, x: 1.0)

  io.println(point_to_string(origin))
  io.println(point_to_string(p))
  io.println(point_to_string(p2))

  let dist_sq = distance_squared(origin, p)
  io.println("distance squared: " <> float.to_string(dist_sq))

  // Gleam supports == for structural equality on all types
  case origin == p {
    True -> io.println("same point")
    False -> io.println("different points")
  }

  let user = User(id: UserId(id: 1), name: "alice", email: "alice@example.com")
  let user = rename(user, "alice smith")
  io.println(user_to_string(user))

  // Gleam uses int.compare for ordering
  let id1 = UserId(id: 1)
  let id2 = UserId(id: 2)
  case int.compare(id1.id, id2.id) {
    order.Lt -> io.println("id1 is smaller")
    _ -> io.println("id2 is smaller or equal")
  }
}
