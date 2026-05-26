# A Real Program

This page walks through a complete, working Tyra program that reads lines from stdin, parses them as CSV-style records, computes statistics, and prints a summary.

## What We're Building

A simple grade calculator that:
1. Reads student records from stdin, one per line, in the format `name,score`
2. Parses each line, skipping malformed input with a warning
3. Computes the count, sum, and average of valid scores
4. Prints the results

Example input:

```
alice,92
bob,85
carol,78
invalid_line
dave,95
```

Expected output:

```
alice: 92
bob: 85
carol: 78
warning: skipping malformed line: invalid_line
dave: 95
---
count:   4
sum:     350
average: 87
```

## The Complete Program

Save this as `grades.tyra` and run with `tyra run grades.tyra < input.txt`:

```tyra
import io
import list
import string

# A parsed student record
value Student
  name: String
  score: Int
end

# Errors that can occur when parsing a line
type ParseError =
  | MissingScore
  | InvalidScore(raw: String)

# Parse a single line in the format "name,score"
fn parse_line(_ line: String) -> Result<Student, ParseError>
  # list.get works on List<Int>; for List<String> from string.split, use for
  mut name = ""
  mut score_str = ""
  mut field = 0
  for part in string.split(line, ",")
    if field == 0
      name = string.trim(part)
    else
      score_str = string.trim(part)
    end
    field = field + 1
  end
  if string.is_empty(name)
    Err(ParseError.MissingScore)
  else
    let score = string.parse_int(score_str).ok_or(
      ParseError.InvalidScore(raw: score_str)
    )?
    Ok(Student(name: name, score: score))
  end
end

# Read all lines from stdin and collect valid scores
fn read_students() -> List<Int>
  mut scores: List<Int> = []
  while true
    match io.read_line()
    when None
      break
    when Some(line)
      let trimmed = string.trim(line)
      if string.is_empty(trimmed)
        # skip blank lines
      else
        match parse_line(trimmed)
        when Ok(student)
          print("#{student.name}: #{student.score}\n")
          scores = list.push(scores, student.score)
        when Err(MissingScore)
          print("warning: skipping malformed line: #{trimmed}\n")
        when Err(InvalidScore(raw))
          print("warning: skipping malformed line: #{trimmed}\n")
        end
      end
    end
  end
  scores
end

fn main() -> Unit
  let scores = read_students()
  let count = list.len(scores)

  print("---\n")
  print("count:   #{count}\n")

  if count == 0
    print("no valid records\n")
  else
    let total = list.sum(scores)
    let avg = total / count
    print("sum:     #{total}\n")
    print("average: #{avg}\n")
  end
end
```

## Walking Through the Code

### Imports

```tyra
import io
import list
import string
```

All imports go at the top of the file. We use `io` for reading stdin, `list` for list operations, and `string` for parsing and trimming.

### The `Student` Value Type

```tyra
value Student
  name: String
  score: Int
end
```

`value` gives us an immutable record with copy semantics. Each parsed record is created once and used read-only — no mutation needed.

### The `ParseError` ADT

```tyra
type ParseError =
  | MissingScore
  | InvalidScore(raw: String)
```

Representing errors as ADT variants lets callers handle each case explicitly. `InvalidScore` carries the raw string so the caller can include it in the warning message.

### Parsing a Line

```tyra
fn parse_line(_ line: String) -> Result<Student, ParseError>
  # list.get works on List<Int>; for List<String> from string.split, use for
  mut name = ""
  mut score_str = ""
  mut field = 0
  for part in string.split(line, ",")
    if field == 0
      name = string.trim(part)
    else
      score_str = string.trim(part)
    end
    field = field + 1
  end
  if string.is_empty(name)
    Err(ParseError.MissingScore)
  else
    let score = string.parse_int(score_str).ok_or(
      ParseError.InvalidScore(raw: score_str)
    )?
    Ok(Student(name: name, score: score))
  end
end
```

Key patterns used here:
- `string.split` returns a `List<String>` — since `list.get` works on `List<Int>` only, we iterate with `for` and track the field index manually
- `.ok_or(error)?` converts `Option` to `Result` and propagates errors with `?`
- The last expression `Ok(student)` is the return value — no `return` keyword needed

### Reading All Students

```tyra
fn read_students() -> List<Int>
  mut scores: List<Int> = []
  while true
    match io.read_line()
    when None
      break
    when Some(line)
      # ...
    end
  end
  scores
end
```

- `mut scores` accumulates results; `list.push` returns a new list on each append
- `io.read_line()` returns `None` at end-of-file, which triggers `break`
- Blank lines are skipped with `string.is_empty`

### Statistics

```tyra
let count = list.len(scores)
let total = list.sum(scores)
let avg = total / count
```

`list.len` and `list.sum` are functions from the `list` module. Integer division is used for the average — extend with `Float` arithmetic when needed.

## Running the Program

**From a file:**

```bash
tyra run grades.tyra < input.txt
```

**Interactive (type records, press Ctrl-D to end input):**

```bash
tyra run grades.tyra
alice,90
bob,82
^D
```

**Build a standalone binary:**

```bash
tyra build -o grades grades.tyra
./grades < input.txt
```

## Extending the Program

Here are some ideas to practice what you have learned:

- **Track the highest and lowest score** — use `list.max` and `list.min`
- **Filter by score range** — count how many students scored above a threshold using `for` and a `mut` counter
- **Read from a file** instead of stdin — use `fs.read_to_string` and `string.split` on the newline character `"\n"`
- **Add a letter grade column** — write a function `fn letter_grade(_ score: Int) -> String` using `match`

## What's Next?

You have covered the core of Tyra. For deeper topics:

- **Standard library reference** — see the source files in `stdlib/` and [Collections](04-collections.md) for the full API
- **Error handling patterns** — see [Error Handling](05-error-handling.md) for `Into`, `defer`, and `panic`
- **Type system** — see [Types and ADTs](06-types-and-adt.md) for `impl`, `Stringable`, and abilities
- **Language specification** — see `docs/spec/ja/language-spec.md` for the full language reference
- **Examples** — see `examples/` for more complete programs including HTTP handlers, JSON parsing, and state machines
