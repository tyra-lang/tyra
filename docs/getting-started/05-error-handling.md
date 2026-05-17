# Error Handling

Tyra has no exceptions. Errors are values — represented by `Option<T>` for optional results and `Result<T, E>` for operations that can fail with a specific error type.

## `Option<T>`

`Option<T>` represents a value that may or may not be present. It has two variants:

- `Some(value)` — the value exists
- `None` — no value

```tyra
import list

let nums = [10, 20, 30]

let first: Option<Int> = list.get(nums, 0)
let missing: Option<Int> = list.get(nums, 99)
```

### Extracting the Value

Use `match` to safely extract the value:

```tyra
import list

fn print_first(_ xs: List<Int>) -> Unit
  match list.get(xs, 0)
  when Some(v)
    print("first: #{v}\n")
  when None
    print("list is empty\n")
  end
end

print_first([1, 2, 3])
print_first([])
```

### The `?` Operator on `Option`

Inside a function that returns `Option<T>`, use `?` to propagate `None` automatically:

```tyra
import string

# If parse_int returns None, the whole function returns None immediately.
fn double_if_int(_ text: String) -> Option<Int>
  let n = string.parse_int(string.trim(text))?
  Some(n * 2)
end

match double_if_int("21")
when Some(n) -> print("doubled: #{n}\n")
when None -> print("not a number\n")
end

match double_if_int("abc")
when Some(n) -> print("doubled: #{n}\n")
when None -> print("not a number\n")
end
```

> **NOTE:** `list.get` returns `Option<Int>` and works on `List<Int>`. To access the first element of a `List<String>` (such as from `string.split`), iterate with `for` — see [Collections](04-collections.md).

### `.ok_or()` — Convert `Option` to `Result`

Convert an `Option<T>` to a `Result<T, E>` by providing an error value for the `None` case:

```tyra
import string

fn parse_positive(_ s: String) -> Result<Int, String>
  let n = string.parse_int(s).ok_or("not a number")?
  if n <= 0
    Err("must be positive")
  else
    Ok(n)
  end
end

match parse_positive("42")
when Ok(n) -> print("got: #{n}\n")
when Err(e) -> print("error: #{e}\n")
end

match parse_positive("abc")
when Ok(n) -> print("got: #{n}\n")
when Err(e) -> print("error: #{e}\n")
end
```

## `Result<T, E>`

`Result<T, E>` represents an operation that either succeeds with a value of type `T` or fails with an error of type `E`. It has two variants:

- `Ok(value)` — success
- `Err(error)` — failure

### Defining Your Error Type

Use a `type` (ADT) to represent the errors your function can produce:

```tyra
type ParseError =
  | InvalidNumber(input: String)
  | OutOfRange(value: Int, min: Int, max: Int)
```

### Returning `Result` from a Function

```tyra
import string

type ParseError =
  | InvalidNumber(input: String)
  | OutOfRange(value: Int, min: Int, max: Int)

fn parse_port(_ s: String) -> Result<Int, ParseError>
  let n = string.parse_int(s).ok_or(ParseError.InvalidNumber(input: s))?
  if n < 1 or n > 65535
    Err(ParseError.OutOfRange(value: n, min: 1, max: 65535))
  else
    Ok(n)
  end
end

match parse_port("8080")
when Ok(port) -> print("port: #{port}\n")
when Err(InvalidNumber(s)) -> print("not a number: #{s}\n")
when Err(OutOfRange(v, lo, hi)) -> print("#{v} out of range #{lo}..#{hi}\n")
end
```

### The `?` Operator on `Result`

Inside a function that returns `Result<T, E>`, `?` on a `Result` either unwraps `Ok(v)` to `v`, or returns `Err(...)` early:

```tyra
import fs
import string

fn read_and_trim(_ path: String) -> Result<String, fs.FsError>
  let content = fs.read_to_string(path)?
  Ok(string.trim(content))
end
```

## File I/O

The `fs` module provides file reading and writing. Both operations return `Result`:

```tyra
import fs

# Read a file
match fs.read_to_string("data.txt")
when Ok(content)
  print("loaded: #{string.len(content)} bytes\n")
when Err(fs.FsError.NotFound(path))
  print("file not found: #{path}\n")
when Err(fs.FsError.PermissionDenied(path))
  print("permission denied: #{path}\n")
when Err(fs.FsError.IoError(msg))
  print("I/O error: #{msg}\n")
end

# Write a file
match fs.write_string("output.txt", "hello\n")
when Ok(_) -> print("written\n")
when Err(_) -> print("write failed\n")
end

# Check existence without reading
if fs.exists("config.toml")
  print("config found\n")
end
```

## Standard Input

The `io` module reads from stdin:

```tyra
import io

# Read one line (trailing newline is stripped)
match io.read_line()
when Some(line)
  print("you typed: #{line}\n")
when None
  print("end of input\n")
end

# Read all remaining stdin at once
let all = io.read_to_end()
print("read #{string.len(all)} bytes\n")
```

> **NOTE:** `io.read_line()` returns `Some("")` for a genuinely empty line and `None` only at end-of-file.

## Writing Functions that Return Errors

A practical pattern for functions that can fail:

```tyra
import io
import string

type InputError =
  | EndOfInput
  | NotANumber(input: String)

fn read_int() -> Result<Int, InputError>
  let line = io.read_line().ok_or(InputError.EndOfInput)?
  let trimmed = string.trim(line)
  string.parse_int(trimmed).ok_or(InputError.NotANumber(input: trimmed))
end

fn main() -> Unit
  match read_int()
  when Ok(n)
    print("you entered: #{n}\n")
  when Err(EndOfInput)
    print("no input\n")
  when Err(NotANumber(s))
    print("not a number: #{s}\n")
  end
end
```

## Next Steps

Continue to [Types and ADTs](06-types-and-adt.md) to learn how to define your own data types.
