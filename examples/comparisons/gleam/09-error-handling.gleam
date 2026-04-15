// 09-error-handling.gleam
// Error handling patterns.
// Gleam has Result as first-class, use expressions for monadic chaining,
// and panic for unrecoverable errors. No defer — Gleam is on BEAM (GC handles cleanup).

import gleam/int
import gleam/io
import gleam/result

pub type ConfigError {
  FileNotFound(path: String)
  ParseError(message: String)
  InvalidValue(key: String, value: String)
}

pub type AppError {
  Config(inner: ConfigError)
  Unexpected(message: String)
}

fn config_to_app_error(err: ConfigError) -> AppError {
  Config(inner: err)
}

// Gleam has no defer — BEAM's GC handles cleanup.
// File operations return Result, chained with use.
fn read_config(path: String) -> Result(String, ConfigError) {
  // simplifile.read returns Result(String, FileError)
  case simplifile.read(path) {
    Ok(content) -> Ok(content)
    Error(_) -> Error(FileNotFound(path: path))
  }
}

fn parse_port(config: String) -> Result(Int, ConfigError) {
  use port <- result.try(
    int.parse(config)
    |> result.replace_error(ParseError(message: "port must be an integer")),
  )

  // Gleam uses Bool conditions normally
  case port < 1 || port > 65_535 {
    True ->
      Error(InvalidValue(key: "port", value: int.to_string(port)))
    False -> Ok(port)
  }
}

fn start_server(port: Int) -> Result(Nil, AppError) {
  case port == 0 {
    True -> panic as "port must not be zero"
    False -> {
      io.println("starting server on port " <> int.to_string(port))
      Ok(Nil)
    }
  }
}

pub fn main() {
  let result = {
    use config <- result.try(
      read_config("app.conf")
      |> result.map_error(config_to_app_error),
    )
    use port <- result.try(
      parse_port(config)
      |> result.map_error(config_to_app_error),
    )
    start_server(port)
  }

  case result {
    Ok(_) -> Nil
    Error(err) -> io.println("error occurred")
  }
}
