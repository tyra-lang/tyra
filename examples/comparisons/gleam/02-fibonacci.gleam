// 02-fibonacci.gleam
// Recursive Fibonacci.

import gleam/io
import gleam/int

fn fib(n: Int) -> Int {
  case n {
    0 -> 0
    1 -> 1
    _ -> fib(n - 1) + fib(n - 2)
  }
}

pub fn main() {
  let result = fib(10)
  io.println("fib(10) = " <> int.to_string(result))
}
