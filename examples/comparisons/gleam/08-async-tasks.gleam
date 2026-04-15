// 08-async-tasks.gleam
// Concurrent task spawning.
// Gleam runs on the BEAM (Erlang VM) — concurrency uses OTP processes,
// not async/await. There is no Task<T> or .await syntax.

import gleam/erlang/process.{type Subject}
import gleam/http/request
import gleam/httpc
import gleam/io
import gleam/list
import gleam/result

pub type FetchError {
  NetworkError(message: String)
  Timeout
  NotFound
}

fn fetch(url: String) -> Result(String, FetchError) {
  let assert Ok(req) = request.to(url)
  case httpc.send(req) {
    Ok(resp) -> Ok(resp.body)
    Error(_) -> Error(NetworkError(message: "request failed"))
  }
}

// Gleam uses Erlang processes for concurrency, not async/await.
// Tasks are spawned as lightweight processes with message passing.
fn fetch_all(urls: List(String)) -> Result(List(String), FetchError) {
  // Spawn a process for each URL
  let tasks =
    list.map(urls, fn(url) {
      process.new_subject()
      |> fn(subject) {
        process.start(fn() { process.send(subject, fetch(url)) }, True)
        subject
      }
    })

  // Receive results from all processes
  list.try_map(tasks, fn(subject) {
    process.receive(subject, 5000)
    |> result.replace_error(Timeout)
    |> result.flatten
  })
}

pub fn main() {
  let urls = [
    "https://api.example.com/a",
    "https://api.example.com/b",
    "https://api.example.com/c",
  ]

  case fetch_all(urls) {
    Ok(results) ->
      list.each(results, io.println)
    Error(_) ->
      io.println("fetch failed")
  }
}
