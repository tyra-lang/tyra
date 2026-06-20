# The Tyra Programming Language: designing for interpretive consistency

Here is a small, complete Tyra program. It compiles, and you can run it in your browser. It is a pricing model: three plans, a function that computes the monthly cost, and a `main` that prints each one.

```tyra
# A tiny pricing model in Tyra: algebraic data types, exhaustive
# matching, errors as values (Result), and no null anywhere.
type Plan =
  | Free
  | Pro(seats: Int)
  | Enterprise(seats: Int, discount: Int)

fn monthly_cost(plan: Plan) -> Result<Int, String>
  match plan
  when Free
    Ok(0)

  when Pro(seats)
    Ok(seats * 20)

  when Enterprise(seats, discount)
    if discount < 0 or discount > 100
      Err("discount #{discount}% is out of range")
    else
      Ok(((seats * 15) * (100 - discount)) / 100)
    end
  end
end

fn main() -> Unit
  let plans = [Plan.Free, Plan.Pro(seats: 5), Plan.Enterprise(seats: 50, discount: 20)]
  for plan in plans
    match monthly_cost(plan)
    when Ok(cost)
      println("$#{cost}/mo")

    when Err(msg)
      println("error: #{msg}")
    end
  end
end
```

It prints `$0/mo`, `$100/mo`, `$600/mo`.

Tyra is a statically-typed, Ruby-flavored language that compiles to native binaries through LLVM, with a tracing garbage collector. It reads like Ruby and ships like Go. It is meant for backend services, CLI tools, and internal business apps — not kernels, not browser frontends, not the places where Rust's borrow checker earns its cost.

Several things are doing work in those 37 lines, and each one is a deliberate choice.

`Plan` is an algebraic data type. The `match` on it is exhaustive: if you add a fourth plan and forget a branch, the program does not compile. There is no `default` case quietly absorbing the variant you forgot.

`monthly_cost` returns `Result<Int, String>`. An out-of-range discount is not an exception thrown up the stack, and it is not a sentinel `-1`. It is a value — `Err("...")` — that the caller has to handle, because the type says so. The same applies to absence: there is no `null` in Tyra. A thing that might not be there is an `Option<T>`, and the type system makes you open the box before you use what is inside.

`Pro(seats: 5)` and `Enterprise(seats: 50, discount: 20)` carry argument labels at the call site, borrowed from Swift. You do not have to remember that the second number is the discount; the call says so. And blocks close with `end`, not braces. Indentation is for humans; it carries no meaning to the compiler.

None of these features is novel on its own. Exhaustive matching is from ML and Rust, `Result` is from Rust, argument labels are from Swift, `end` blocks and `#{...}` interpolation are from Ruby. What is worth explaining is not the features. It is the principle they were selected to serve.

## Why: interpretive consistency

Tyra has one organizing idea. The same input should yield the same parse, the same types, and the same meaning — every time, for every reader. I call this *interpretive consistency*, and the reader I have in mind is not only a person. It is anything that has to interpret the code: the compiler, the formatter, the language server, the next maintainer, and increasingly a language model.

Ambiguity is the enemy of all of them. When a language offers three ways to write the same thing, every reader has to hold all three in mind. When a value can be silently coerced, every reader has to track the coercion. When a name can be a method call or a variable depending on context, every reader has to resolve the context first. These are small costs paid on every single line.

So Tyra removes the ambiguity at the source, and accepts the verbosity that costs.

There are no implicit conversions. An `Int` does not become a `Float` because the expression around it wanted one; you convert it yourself. There is no truthy or falsy. An `if` condition must be a `Bool` — not zero, not the empty string, not `None`. The logical operators are the keywords `and`, `or`, `not`, and both sides must be `Bool`. Function calls always take parentheses, even with no arguments: `now()`, never `now`. There is, as much as a language can manage, one way to write each thing.

The sharpest example of the principle is the part of Tyra's design I consider genuinely its own: the separation of **traits** from **abilities**.

A trait is replaceable behavior. You write an `impl` to provide it, and different types provide it differently — this is the ordinary interface mechanism you would expect.

An ability is a structural property the compiler already knows how to check: `Eq`, `Hash`, `Ord`, `Debug`. You cannot write an `impl` for an ability. There is no place to put a clever, wrong equality. Abilities are derived by rule from a type's shape: a value type gets `Eq` when all its fields have `Eq`, gets `Hash` when all its fields have `Hash`, and so on. The rule is the same everywhere, so the answer to "can I compare these two values?" does not depend on whether someone remembered to derive something.

This is also why `Float` has no `Eq`, and therefore no `==`. IEEE 754 says `NaN` is not equal to itself. If `Float` had a structural equality ability, that ability would have to either lie about `NaN` or propagate the contradiction into every type that contains a float. So `Float` does not get the ability. Comparing two floats takes an explicit call into the standard library. The cost is real, and the cost is the point: the `NaN == NaN` trap cannot be written by accident. (There is a longer write-up of just this decision, "Why Float Has No `==` in Tyra," if the tradeoff interests you.)

## Does it work?

A design goal is a hypothesis. The hypothesis here is that removing ambiguity makes Tyra easier to interpret correctly. People are hard to measure. A language model is not.

So I ran one. The benchmark takes 100 plain-English programming tasks — none of which mention Tyra, or any language — and asks a frontier model to write each one. The output is graded in three stages: it must generate, it must compile and type-check, and it must run and produce the expected output markers. There is no partial credit and no human in the loop fixing the code afterward.

Tyra has zero presence in any model's training data; it is too new and too small. So for Tyra the harness injects the full language spec, the canonical example programs, and the entire standard library source into the system prompt. This is the whole experiment: can a model that has never seen Tyra write correct Tyra from the specification alone? Without that injected context, the score is zero — the model does not know the syntax. This is disclosed, and it is the honest framing: the result measures how learnable the language is from its spec, not how famous it is.

On the current v0.11.0 compiler (the run is recorded as "run56"), the model writes Tyra that passes on the first try **88.7% of the time** — a mean of 266 passes across 300 runs, three seeds over the 100 tasks. Counted by task, 98% pass on at least one of the three seeds, and 77% pass on all three.

A few honest boundaries on that number. The grader checks output markers, not full correctness — a program with the right markers and wrong internals would pass. The exact model behind run56 is not pinned in the stored artifacts, so I do not attach a model name to it. And while I have rough single-seed figures for other languages, they come from a separate, earlier run against an older compiler; they are directional context, not a same-condition comparison, and I am not claiming Tyra beats anything. A controlled, multi-seed, cross-language sweep is the obvious next step and it is still pending. The full method, caveats, and reproduction steps are in `bench/ai-gen/METHODOLOGY.md`.

What I will claim is narrow: a design built to remove ambiguity for humans appears to remove it for machines too. That is correlation and a prediction, not proof. It is also exactly what the design predicted.

## A personal note

I build Tyra alone. I reach for Ruby when I want a program to read like a sentence, and I reach for a compiler when I want the machine to catch my mistakes before a user does. For years those two wants pulled in opposite directions. Tyra is my attempt to stop choosing.

The interpretive-consistency bet is also a bet about how code gets written now. I write a growing share of my own code alongside a model, and the languages that frustrate that collaboration are exactly the ones that frustrate a new human teammate: too many ways to say one thing, too much that is implied rather than stated. Designing for one reader turned out to be designing for all of them. That is the wager the whole language is built on.

## Try it

The showcase above runs in the browser, no install:

- Playground: https://tyra-lang.github.io/playground/?sample=showcase&run=1
- Site: https://tyra-lang.github.io

To install:

```sh
curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh
# or
brew install tyra-lang/tap/tyra
```

Prebuilt for macOS (arm64) and Linux x86_64 (glibc and static musl). Windows is experimental. One binary gives you the whole toolchain — `check`, `run`, `build`, `fmt`, `test`, `new`, `mod` — plus an LSP server, a VS Code extension, and a DAP debugger.

Be honest with yourself about where it stands: Tyra is pre-1.0, at v0.11.0, Apache-2.0, and maintained by one person. Before 1.0, breaking changes can land in minor versions. The bundled `http.server` is experimental — single-threaded, no TLS, no middleware — and not for production. This is a language you can read, run, and form an opinion on today, not one you should bet a company on tomorrow.

If the trait/ability separation, or the missing `==` on `Float`, or the no-null insistence makes you want to argue, that is the right reaction. The spec is short and complete, the repository is open, and the issue tracker is where I would rather have the argument than in the abstract:

- Repository: https://github.com/tyra-lang/tyra
