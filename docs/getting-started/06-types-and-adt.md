# Types and ADTs

Tyra provides two ways to define structured data types — `type` (algebraic data types / ADTs) and `value`/`data` (record types). This page covers all three.

## Algebraic Data Types with `type`

An ADT (`type`) defines a closed set of variants. Each variant can carry fields:

```tyra
type Color =
  | Red
  | Green
  | Blue

type Shape =
  | Circle(radius: Float)
  | Rect(width: Float, height: Float)
  | Point
```

### Constructing Variants

Use the qualified form `TypeName.Variant(...)` to construct a variant:

```tyra
let c = Color.Red
let s = Shape.Circle(radius: 5.0)
let r = Shape.Rect(width: 3.0, height: 4.0)
```

> **NOTE:** `Some`, `None`, `Ok`, and `Err` are the only variants that do not require qualification — they are part of the language prelude.

### Matching on Variants

`match` patterns use the **unqualified** variant name inside `when`:

```tyra
fn describe(_ s: Shape) -> String
  match s
  when Circle(r)
    "circle with radius #{r}"
  when Rect(w, h)
    "rect #{w} x #{h}"
  when Point
    "point"
  end
end

print("#{describe(Shape.Circle(radius: 3.0))}\n")
print("#{describe(Shape.Rect(width: 2.0, height: 5.0))}\n")
```

### Computing with ADTs

```tyra
fn area(_ s: Shape) -> Float
  match s
  when Circle(r) -> 3.14159 * r * r
  when Rect(w, h) -> w * h
  when Point -> 0.0
  end
end
```

## `value` Types

A `value` type is an **immutable record**. All fields are fixed after construction. Value types have copy semantics — they behave like integers, not objects.

```tyra
value Point
  x: Float
  y: Float
end

let p = Point(x: 3.0, y: 4.0)
print("#{p.x}, #{p.y}\n")
```

### `copy()` — Create a Modified Copy

Use `.copy(field: newValue)` to produce a new `value` with some fields changed:

```tyra
value Point
  x: Float
  y: Float
end

let p = Point(x: 1.0, y: 2.0)
let q = p.copy(x: 10.0)   # new Point with x=10.0, y=2.0
print("#{q.x}, #{q.y}\n")
```

### Implementing `Stringable`

Implement the `Stringable` trait to make your type usable inside `#{...}`:

```tyra
value Point
  x: Float
  y: Float
end

impl Stringable for Point
  fn to_string(self) -> String
    "(#{self.x}, #{self.y})"
  end
end

let p = Point(x: 3.0, y: 4.0)
print("point: #{p}\n")
```

## `data` Types

A `data` type is a **mutable reference type**. Fields marked `mut` can be updated after construction. The GC manages the memory.

```tyra
data Counter
  mut value: Int
end

mut c = Counter(value: 0)
c.value = c.value + 1
c.value = c.value + 1
print("count: #{c.value}\n")
```

> **NOTE:** `data` is a reference type. When you pass a `data` value to a function, the function receives a reference to the same object. Mutations inside the function are visible to the caller.

### Mutating in a Function

Function parameters are immutable bindings. To mutate a `data` field, rebind the parameter to `mut` inside the function body:

```tyra
data User
  id: Int
  mut name: String
end

fn rename(_ user: User, _ new_name: String) -> Unit
  mut u = user          # rebind to a mut binding (same object)
  u.name = new_name     # mutates the shared object
end

mut alice = User(id: 1, name: "alice")
rename(alice, "alice smith")
print("#{alice.name}\n")   # prints: alice smith
```

## Combining ADTs with `value` and `data`

```tyra
type Status =
  | Active
  | Suspended(reason: String)
  | Deleted

data Account
  id: Int
  mut status: Status
end

fn suspend(_ acc: Account, _ reason: String) -> Unit
  mut a = acc
  a.status = Status.Suspended(reason: reason)
end

mut acc = Account(id: 42, status: Status.Active)
suspend(acc, "payment overdue")

match acc.status
when Active -> print("active\n")
when Suspended(r) -> print("suspended: #{r}\n")
when Deleted -> print("deleted\n")
end
```

## Recursive Data Structures

ADTs can be recursive, enabling tree-like structures:

```tyra
type Tree =
  | Leaf(value: Int)
  | Node(left: Tree, right: Tree)

fn tree_sum(_ t: Tree) -> Int
  match t
  when Leaf(v) -> v
  when Node(l, r) -> tree_sum(l) + tree_sum(r)
  end
end

fn tree_depth(_ t: Tree) -> Int
  match t
  when Leaf(_) -> 1
  when Node(l, r)
    let dl = tree_depth(l)
    let dr = tree_depth(r)
    if dl > dr
      dl + 1
    else
      dr + 1
    end
  end
end

let tree = Tree.Node(
  left: Tree.Node(
    left: Tree.Leaf(value: 1),
    right: Tree.Leaf(value: 2)
  ),
  right: Tree.Leaf(value: 3)
)

print("sum:   #{tree_sum(tree)}\n")
print("depth: #{tree_depth(tree)}\n")
```

## `impl` — Adding Methods

Use `impl` to add methods to any type. The `self` parameter refers to the receiver:

```tyra
type Shape =
  | Circle(radius: Float)
  | Rect(width: Float, height: Float)

impl Shape
  fn area(self) -> Float
    match self
    when Circle(r) -> 3.14159 * r * r
    when Rect(w, h) -> w * h
    end
  end

  fn description(self) -> String
    match self
    when Circle(r) -> "circle(r=#{r})"
    when Rect(w, h) -> "rect(#{w}x#{h})"
    end
  end
end

let s = Shape.Circle(radius: 6.0)
print("#{s.description()}: area = #{s.area()}\n")

let r = Shape.Rect(width: 4.0, height: 5.0)
print("#{r.description()}: area = #{r.area()}\n")
```

> **TIP:** The free-function style (`area(shape)`) and the method style (`shape.area()`) are both valid Tyra. Methods are syntactic sugar — choose whichever reads more clearly at the call site.

## Summary

| Keyword | Description |
|---|---|
| `type` | ADT with named variants; matched with `when` |
| `value` | Immutable record; copy semantics; supports `copy()` |
| `data` | Mutable record; reference semantics; GC-managed |
| `impl` | Attach methods to any type |

## Next Steps

Continue to [A Real Program](07-real-program.md) for a complete working example that ties everything together.
