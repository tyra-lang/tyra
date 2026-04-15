// 03-option-result.gleam
// Option and Result usage patterns.
// Gleam uses Option (from gleam/option) and Result (built-in).

import gleam/io
import gleam/option.{type Option, None, Some}
import gleam/result

pub type LookupError {
  NotFound
  InvalidId
}

fn find_user(id: Int) -> Option(String) {
  case id {
    1 -> Some("alice")
    2 -> Some("bob")
    _ -> None
  }
}

// Gleam has use for monadic chaining, similar to ? but different.
fn user_greeting(id: Int) -> Option(String) {
  use name <- option.map(find_user(id))
  "hello, " <> name
}

// Convert Option to Result with option.to_result
fn get_user_result(id: Int) -> Result(String, LookupError) {
  case id <= 0 {
    True -> Error(InvalidId)
    False -> {
      find_user(id)
      |> option.to_result(NotFound)
    }
  }
}

pub fn main() {
  case user_greeting(1) {
    Some(msg) -> io.println(msg)
    None -> io.println("not found")
  }

  case get_user_result(0) {
    Ok(name) -> io.println("found: " <> name)
    Error(NotFound) -> io.println("user not found")
    Error(InvalidId) -> io.println("invalid id")
  }
}
