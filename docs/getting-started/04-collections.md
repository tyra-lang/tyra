# Collections

Tyra's standard library provides `List<T>` via the `list` module, string utilities via the `string` module, `Map<K, V>` for key-value lookups, and `Set<T>` for membership testing.

## `List<T>`

A `List<T>` is an ordered, immutable sequence. All list operations return new lists — the original is never modified.

> **NOTE:** `list.get`, `list.sum`, `list.max`, and `list.min` operate on `List<Int>`. `List<String>` from `string.split` supports `list.len`, `list.push`, and `for` iteration.

### List Literals

```tyra
let nums = [1, 2, 3, 4, 5]
let empty: List<Int> = []
```

### `list.len` — Length

```tyra
import list

let nums = [10, 20, 30]
let n = list.len(nums)
print("length: #{n}\n")
```

### `list.get` — Safe Access

`list.get` returns `Option<Int>` — it never panics on out-of-bounds access:

```tyra
import list

let nums = [10, 20, 30]

match list.get(nums, 0)
when Some(v)
  print("first: #{v}\n")
when None
  print("out of range\n")
end
```

> **NOTE:** Direct index access (`nums[5]`) panics at runtime on out-of-bounds. Prefer `list.get` for safe access.

### `list.push` — Append

`list.push` returns a **new** list with the element appended. The original list is unchanged:

```tyra
import list

let nums = [1, 2, 3]
let nums2 = list.push(nums, 4)
let nums3 = list.push(nums2, 5)

print("original length: #{list.len(nums)}\n")
print("new length: #{list.len(nums3)}\n")
```

### `list.sum`, `list.max`, `list.min`

```tyra
import list

let scores = [85, 92, 78, 95, 88]

let total = list.sum(scores)
print("sum: #{total}\n")

match list.max(scores)
when Some(m) -> print("max: #{m}\n")
when None -> print("empty\n")
end

match list.min(scores)
when Some(m) -> print("min: #{m}\n")
when None -> print("empty\n")
end
```

### `list.contains` and `list.index_of`

```tyra
import list

let primes = [2, 3, 5, 7, 11]

if list.contains(primes, 7)
  print("7 is prime\n")
end

match list.index_of(primes, 5)
when Some(i) -> print("5 is at index #{i}\n")
when None -> print("not found\n")
end
```

### Iterating with `for`

```tyra
import list

let nums = [1, 2, 3, 4, 5]

for x in nums
  print("#{x}\n")
end

# Accumulate with a mutable variable
mut total = 0
for x in nums
  total = total + x
end
print("total: #{total}\n")
```

## String Utilities

The `string` module provides common string operations. Import it at the top of your file:

```tyra
import string
```

### `string.len` — Byte Length

```tyra
import string

let s = "hello"
print("length: #{string.len(s)}\n")
```

> **NOTE:** `string.len` returns the UTF-8 byte length, not the number of Unicode characters. `string.len("あ")` returns `3`.

### `string.trim`

Removes leading and trailing ASCII whitespace:

```tyra
import string

let s = "  hello  "
let trimmed = string.trim(s)
print("'#{trimmed}'\n")
```

### `string.to_ascii_upper` / `string.to_ascii_lower`

ASCII case conversion:

```tyra
import string

print("#{string.to_ascii_upper("hello")}\n")
print("#{string.to_ascii_lower("WORLD")}\n")
```

### `string.contains`, `string.starts_with`, `string.ends_with`

```tyra
import string

let path = "/home/alice/config.toml"

if string.ends_with(path, ".toml")
  print("TOML file\n")
end

if string.starts_with(path, "/home")
  print("home directory\n")
end

if string.contains(path, "alice")
  print("alice's file\n")
end
```

### `string.parse_int` — String to Int

Converts a string to an integer, returning `Option<Int>`:

```tyra
import string

match string.parse_int("42")
when Some(n) -> print("parsed: #{n}\n")
when None -> print("not a number\n")
end

match string.parse_int("abc")
when Some(n) -> print("parsed: #{n}\n")
when None -> print("not a number\n")
end
```

> **TIP:** Leading/trailing whitespace causes `parse_int` to return `None`. Use `string.trim` first if needed.

### `string.split` and `string.split_whitespace`

Split a string into a `List<String>`:

```tyra
import string

# Split on a separator character
let parts = string.split("a,b,c", ",")
for p in parts
  print("part: #{p}\n")
end

# Split on whitespace (adjacent spaces are collapsed)
let words = string.split_whitespace("one two   three")
for w in words
  print("word: #{w}\n")
end
```

### `string.substring` — Byte Slice

Extract a substring using a half-open byte range `[start, stop)`:

```tyra
import string

let s = "Hello, World!"
let hello = string.substring(s, 0, 5)
print("#{hello}\n")
```

> **NOTE:** Indexing is byte-based. Slicing inside a multi-byte UTF-8 character returns an empty string.

## Building a List Incrementally

Since lists are immutable, build them by accumulating with `list.push`:

```tyra
import list
import string

fn parse_ints(_ input: String) -> List<Int>
  let parts = string.split(input, ",")
  mut result: List<Int> = []
  for p in parts
    let trimmed = string.trim(p)
    match string.parse_int(trimmed)
    when Some(n)
      result = list.push(result, n)
    when None
      # skip non-integer tokens
    end
  end
  result
end

let numbers = parse_ints("1, 2, 3, 4, 5")
print("count: #{list.len(numbers)}\n")
print("sum:   #{list.sum(numbers)}\n")
```

## `Map<K, V>`

A `Map<K, V>` stores key-value pairs. Keys must support equality and hashing (`Eq + Hash`); values can be any type. Primitive types (`Int`, `Bool`, `String`) satisfy `Eq + Hash` automatically; `Float` does not.

### Map Literals

Build a map with a literal. The type is inferred from the elements:

```tyra
let scores: Map<String, Int> = {"alice": 92, "bob": 85}
let flags: Map<String, Bool> = {"debug": true, "verbose": false}
let table: Map<Int, Int> = {1: 100, 2: 200}
```

An empty map literal requires an explicit type annotation:

```tyra
let empty: Map<String, Int> = {}
```

### `.get` — Safe Lookup

`.get` returns `Option<V>` — it never panics on missing keys:

```tyra
let scores: Map<String, Int> = {"alice": 92, "bob": 85}

match scores.get("alice")
when Some(n)
  print("score: #{n}\n")
when None
  print("not found\n")
end
```

### `.contains_key` — Existence Check

```tyra
if scores.contains_key("bob")
  print("bob is present\n")
end
```

### `.len` — Size

```tyra
print("entries: #{scores.len()}\n")
```

> **NOTE:** Maps are immutable — there is no `insert` or `remove` method. To build a map with all entries known at once, use a literal. For dynamic keyed data, use a list of value types (see [Types and ADTs](06-types-and-adt.md)).

## `Set<T>`

A `Set<T>` is an unordered collection of unique elements. Elements must support `Eq + Hash`; primitive types satisfy this automatically.

### Creating a Set

Import the `set` module and call `set.new()`. A type annotation is required when the element type cannot be inferred from context:

```tyra
import set

let s: Set<Int> = set.new()
```

### `.insert` — Adding Elements

`.insert` returns a **new** set with the element added (the original is unchanged). Inserting a duplicate has no effect:

```tyra
import set

let s: Set<Int> = set.new()
let s1 = s.insert(1)
let s2 = s1.insert(2)
let s3 = s2.insert(1)   # duplicate — no change

print("size: #{s3.len()}\n")   # 2
```

### `.contains` — Membership Test

```tyra
if s3.contains(2)
  print("2 is in the set\n")
end
```

### `.len` — Size

```tyra
print("size: #{s3.len()}\n")
```

> **NOTE:** `Float` does not have `Eq` and cannot be used as a `Set` element. For custom `value` types, `Eq` and `Hash` are derived automatically if all fields are hashable.

## Next Steps

Continue to [Error Handling](05-error-handling.md) to learn about `Option<T>`, `Result<T, E>`, and the `?` operator.
