// 05-json-parsing.gleam
// JSON parsing with custom error types.
// Gleam uses gleam/json and gleam/dynamic for JSON.

import gleam/dynamic.{type Dynamic}
import gleam/io
import gleam/json
import gleam/result

pub type JsonError {
  ParseFailed(message: String)
  TypeMismatch(expected: String, got: String)
  MissingKey(key: String)
}

pub type AppError {
  Json(inner: JsonError)
  Io(message: String)
}

fn json_to_app_error(err: JsonError) -> AppError {
  Json(inner: err)
}

fn parse_name(doc: Dynamic) -> Result(String, JsonError) {
  doc
  |> dynamic.field("name", dynamic.string)
  |> result.map_error(fn(_) { MissingKey(key: "name") })
}

fn load_user_name(input: String) -> Result(String, AppError) {
  let doc =
    json.decode(input, dynamic.dynamic)
    |> result.map_error(fn(_) { Json(inner: ParseFailed(message: "invalid json")) })

  use d <- result.try(doc)

  parse_name(d)
  |> result.map_error(json_to_app_error)
}

pub fn main() {
  case load_user_name("{\"name\": \"alice\"}") {
    Ok(name) -> io.println("user: " <> name)
    Error(Json(inner: MissingKey(key: k))) ->
      io.println("missing key: " <> k)
    Error(Json(inner: TypeMismatch(expected: exp, got: got))) ->
      io.println("type error: expected " <> exp <> ", got " <> got)
    Error(Json(inner: ParseFailed(message: msg))) ->
      io.println("parse failed: " <> msg)
    Error(Io(message: msg)) ->
      io.println("io error: " <> msg)
  }
}
