# Competitive Landscape

> Quick reference for README and external communications.
> For the full strategic analysis (acquisition strategy, risk analysis, roadmap, decision framework), see [`docs/strategy.md`](strategy.md).
> For AI assistant guidance, see [`AGENTS.md`](../AGENTS.md).

Tyra positions itself in a 5-layer competitive landscape:

## Direct design competitor: Crystal

Crystal occupies the same surface position (Ruby-like syntax + static typing + LLVM-native compilation). Tyra differentiates by removing macros, operator overloading, and runtime reflection that Crystal keeps for Ruby compatibility.

## Strategic benchmark: Go

Go is the gold standard for operational simplicity (gofmt, go test, go mod, single binary). Tyra borrows this operational model as a quality benchmark, not as a market to displace.

## Philosophical competitor: Gleam

Gleam shares Tyra's commitment to type safety, Result-based error handling, and AI-friendly determinism. The difference is execution target (BEAM/JS vsLLVM-native) and programming style (functional vs imperative).

## Message-space competitor: V

V's marketing (simple, fast, safe, compiled, no null, Option/Result) overlaps significantly with Tyra's. Differentiation must come from stricter semantic constraints and team-deployable convention fixity.

## Syntactic ancestor: Ruby

Tyra borrows readability conventions (end blocks, #{} interpolation, match/when) from Ruby but rejects Ruby's dynamic flexibility, metaprogramming, and implicit receivers. Ruby users approaching Tyra must adjust expectations: Tyra is not a Ruby successor.

## Tyra in one sentence

Tyra is a Ruby-readable native language that strips Crystal's metaprogramming, mirrors Go's operational simplicity, and constrains itself more strictly than Gleam or V — designed to be auditable by both humans and AI.
