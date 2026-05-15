# Control Flow

Tyra provides `if/else`, `for`, `while`, and `match` for controlling the flow of execution. All blocks are closed with `end`.

## `if` / `else`

```tyra
let x = 10

if x > 0
  print("positive\n")
else
  print("non-positive\n")
end
```

`else if` chains are supported with a single `end`:

```tyra
fn classify(_ n: Int) -> String
  if n > 0
    "positive"
  else if n < 0
    "negative"
  else
    "zero"
  end
end

print("#{classify(5)}\n")
print("#{classify(-3)}\n")
print("#{classify(0)}\n")
```

`if/else` is an expression — it produces a value:

```tyra
let label = if x % 2 == 0
  "even"
else
  "odd"
end
print("#{x} is #{label}\n")
```

> **NOTE:** The condition must be a `Bool`. Tyra has no truthy/falsy — `if 0` or `if ""` are compile errors.

## `for` Loop

Iterate over a list with `for ... in ... end`:

```tyra
import list

let nums = [1, 2, 3, 4, 5]

for x in nums
  print("#{x}\n")
end
```

You can also iterate over a `List<String>`:

```tyra
import string

let words = string.split_whitespace("one two three")

for w in words
  print("word: #{w}\n")
end
```

> **NOTE:** `for` loops do not expose an index variable. If you need the index, use a `mut` counter alongside:

```tyra
mut i = 0
for x in nums
  print("#{i}: #{x}\n")
  i = i + 1
end
```

## `while` Loop

Repeat a block while a condition is `true`:

```tyra
mut n = 1
while n <= 5
  print("#{n}\n")
  n = n + 1
end
```

## `break`

Exit a `for` or `while` loop early with `break`:

```tyra
mut i = 0
while true
  if i >= 3
    break
  end
  print("#{i}\n")
  i = i + 1
end
```

```tyra
import list

let items = [10, 20, 0, 30]

for v in items
  if v == 0
    break
  end
  print("#{v}\n")
end
```

## `match`

`match` compares a value against a series of patterns. Every `match` must be exhaustive — all possible cases must be handled:

```tyra
fn fib(_ n: Int) -> Int
  match n
  when 0
    0
  when 1
    1
  when _
    fib(n - 1) + fib(n - 2)
  end
end

println("fib(10) = #{fib(10)}")
```

`_` is the wildcard pattern that matches anything not covered by earlier branches.

### Matching on Strings

```tyra
fn day_type(_ day: String) -> String
  match day
  when "Saturday"
    "weekend"
  when "Sunday"
    "weekend"
  when _
    "weekday"
  end
end

print("#{day_type("Monday")}\n")
print("#{day_type("Saturday")}\n")
```

### Matching on ADT Variants

`match` is most powerful when used with algebraic data types. See [Types and ADTs](06-types-and-adt.md) and [Error Handling](05-error-handling.md) for detailed examples. A quick preview:

```tyra
import list

let items = [1, 2, 3]
let first = list.get(items, 0)

match first
when Some(v)
  print("first item: #{v}\n")
when None
  print("list is empty\n")
end
```

### `match` as an Expression

Like `if/else`, `match` is an expression and produces a value. Use the `->` inline form for single-expression arms:

```tyra
import list

let nums = [4, 7, 2]
let description = match list.get(nums, 0)
  when Some(v) -> "starts with #{v}"
  when None -> "empty list"
end
print("#{description}\n")
```

> **TIP:** When each arm is a single expression, the inline `->` form is compact. When arms need multiple statements, use the indented block form (no `->`) as shown in the `fib` example above.

## Combining Conditions

Use `and`, `or`, and `not` (keywords, not operators) to build compound boolean conditions:

```tyra
let x = 15

if x > 10 and x < 20
  print("in range\n")
end

if x < 0 or x > 100
  print("out of bounds\n")
end

if not (x == 0)
  print("non-zero\n")
end
```

## Next Steps

Continue to [Collections](04-collections.md) to learn about `List<T>` and string utilities.
