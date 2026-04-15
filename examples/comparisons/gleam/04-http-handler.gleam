// 04-http-handler.gleam
// HTTP server with multiple route handlers.
// Gleam uses mist + wisp or gleam/http for HTTP.
// This example uses wisp, the most common Gleam web framework.

import gleam/http/request.{type Request}
import gleam/http/response.{type Response}
import gleam/string_builder
import mist
import wisp.{type Request as WispRequest}

pub type AppError {
  ServerError(message: String)
  HandlerError(message: String)
}

fn health_handler(_req: WispRequest) -> Response(String) {
  response.new(200)
  |> response.set_body("ok")
}

fn greet_handler(req: WispRequest) -> Response(String) {
  case wisp.get_query(req, "name") {
    Ok(name) ->
      response.new(200)
      |> response.set_body("hello, " <> name)
    Error(_) ->
      response.new(400)
      |> response.set_body("missing name")
  }
}

fn router(req: WispRequest) -> Response(String) {
  case wisp.path_segments(req) {
    ["health"] -> health_handler(req)
    ["greet"] -> greet_handler(req)
    _ ->
      response.new(404)
      |> response.set_body("not found")
  }
}

pub fn main() {
  let assert Ok(_) =
    wisp.mist_handler(router, "secret")
    |> mist.new
    |> mist.port(8080)
    |> mist.start_http

  // Gleam: the server runs in an Erlang process; main doesn't block.
  // In practice you'd use erlang.sleep_forever() or a supervisor.
  process.sleep_forever()
}
