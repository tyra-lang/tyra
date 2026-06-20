# Tyra — Hacker News submission package

_AI is intentionally NOT in the title (readability-first). Post the first comment immediately after submitting._

## Candidate titles

- The Tyra Programming Language: designing for interpretive consistency
- Tyra: a readable, statically-typed language that compiles to native binaries
- Tyra: Ruby-flavored syntax, no null, exhaustive match, compiles via LLVM
- Why Tyra separates traits (behavior) from abilities (Eq/Hash/Ord/Debug)

## Author first comment

Author here. I build Tyra alone, and the honest origin is a tension I never resolved: I reach for Ruby when I want a program to read like a sentence, and I reach for a compiler when I want the machine to catch my mistakes before a user does. For years those two wants pulled in opposite directions, and Tyra is my attempt to stop choosing between them — Ruby-style `end` blocks and `#{...}` interpolation on top of static types, no null, and exhaustive `match`.

The one idea underneath all of it is what I call interpretive consistency: the same input should yield the same parse, the same types, and the same meaning, every time, for every reader. So there are no implicit conversions, no truthy/falsy, one way to write each thing. The part I think is genuinely original is splitting traits (replaceable behavior you write an `impl` for) from abilities (structural properties like Eq/Hash/Ord/Debug that the compiler derives by rule and you cannot override). That's also why `Float` has no `==` — there's no place to write a wrong `NaN` equality by accident.

The playground runs the showcase from the post with no install, if you'd rather poke at it than read me describe it: https://tyra-lang.github.io/playground/?sample=showcase&run=1

There's also a benchmark section in the post, and I want to be upfront that it's a proof point, not the pitch — the language has to stand on its own first. The full method, every caveat, and the reproduction steps are here: https://github.com/tyra-lang/tyra/blob/main/bench/ai-gen/METHODOLOGY.md

It's pre-1.0 (v0.11.0), Apache-2.0, one maintainer, and breaking changes can land in minor versions. If the trait/ability split or the missing `==` makes you want to argue, that's the reaction I was hoping for — I'd rather have it here or on the issue tracker than in the abstract.

## Pre-written skeptic replies

### Objection: Why not just use Python/Ruby/Crystal? LLMs already know those cold, so a model writing correct code in a known language is unremarkable — and your own numbers show Ruby at 99%.

You're right, and I say so in the post: Ruby's ~99% reflects enormous training data, not anything about its design, and Tyra has zero presence in any model's training data — so the harness has to inject the full spec, the example programs, and the whole stdlib source just to get a non-zero score. That's the entire experiment, and it's a different question than 'does the model already know this.' The claim isn't 'a model writes correct code' — known languages win that trivially. The claim is narrow: a model that has never seen Tyra can write correct Tyra from the specification alone, which is a measure of how learnable the language is from its spec. And to be explicit, the cross-language figures (Crystal 96%, Go 81%, etc.) are single-seed point estimates from a separate, earlier run on an older compiler — directional context only, not a same-condition comparison, and I'm not claiming Tyra beats Crystal or anything else. If you already work in Crystal or Python and they fit, you should keep using them; Tyra is for people who specifically want the Ruby-reading / static-checking combination it's built around.

### Objection: Yet another programming language from one person. Why does the world need this, and why should anyone invest time in something that'll never reach critical mass?

Fair, and I won't pretend the base rate for solo pre-1.0 languages is good. I'm not asking anyone to bet a company on it — the post says exactly that, and the bundled http.server is experimental (single-threaded, no TLS) and explicitly not for production. What I'd push back on is 'yet another' implying it's a random remix: the features are all borrowed (Result from Rust, labels from Swift, end-blocks from Ruby), but they were selected to serve one specific principle — interpretive consistency, removing ambiguity at the source — and the trait/ability separation is the part I haven't seen done this way elsewhere. The honest pitch is: it's a language you can read, run in the browser, and form an opinion on in ten minutes, not one you should adopt at work tomorrow. If it dies at v0.x, the trait/ability idea is still in the open for anyone to take.

### Objection: Which model, which prompt, and is this reproducible? '88.7%' with no model name pinned reads like a cherry-picked number.

The number is 88.7% mean across 300 runs — 3 seeds over 100 tasks — recorded as run56 on the v0.11.0 compiler; by task it's 98% passing on at least one seed and 77% on all three. I'll own the weak spots directly, because they're real: the exact model behind run56 isn't pinned in the stored artifacts, so I deliberately don't attach a model name to it; and the grader checks output markers, not full correctness, so a program with the right markers and wrong internals would pass — it's strictly stronger than 'it compiled' but weaker than full equivalence. The full procedure, the prompt-neutrality audit (no prompt mentions any language), the three-stage scoring, and reproduction steps are in bench/ai-gen/METHODOLOGY.md, and the prompts are versioned in git so you can inspect every one. The thing I am not claiming is a cross-language win — that requires a controlled multi-seed sweep against the other languages on the same compiler, and that's still pending.
