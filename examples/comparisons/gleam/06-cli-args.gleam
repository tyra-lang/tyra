// 06-cli-args.gleam
// Command-line argument parsing.
// Gleam uses argv package or erlang.start_arguments().

import argv
import gleam/int
import gleam/io
import gleam/list
import gleam/result

pub type CliError {
  MissingCommand
  UnknownCommand(name: String)
  MissingArg(name: String)
  InvalidArg(name: String, value: String)
}

fn run_serve(args: List(String)) -> Result(Nil, CliError) {
  use port_str <- result.try(
    list.at(args, 2)
    |> result.replace_error(MissingArg(name: "port")),
  )
  use port <- result.try(
    int.parse(port_str)
    |> result.replace_error(InvalidArg(name: "port", value: port_str)),
  )
  io.println("serving on port " <> int.to_string(port))
  Ok(Nil)
}

fn run_help() -> Result(Nil, CliError) {
  io.println("usage: myapp <command> [args]")
  io.println("commands:")
  io.println("  serve <port>  start the server")
  io.println("  help          show this message")
  Ok(Nil)
}

pub fn main() {
  let args = argv.load().arguments
  let result = case list.at(args, 0) {
    Error(_) -> Error(MissingCommand)
    Ok("serve") -> run_serve(args)
    Ok("help") -> run_help()
    Ok(name) -> Error(UnknownCommand(name: name))
  }

  case result {
    Ok(_) -> Nil
    Error(err) -> {
      io.println("error: " <> cli_error_to_string(err))
    }
  }
}

fn cli_error_to_string(err: CliError) -> String {
  case err {
    MissingCommand -> "missing command"
    UnknownCommand(name: n) -> "unknown command: " <> n
    MissingArg(name: n) -> "missing arg: " <> n
    InvalidArg(name: n, value: v) -> "invalid arg " <> n <> ": " <> v
  }
}
