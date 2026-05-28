# ai-gen benchmark summary

Prompts observed: 100

Latest sweep: **Run 17** (`tyra+spec × claude × 100`, seed=2, 2026-05-28)
after v0.7.0 post-release hardening: E0204 promoted to hard compile error
(`Program::lower_errors`), E0213 new error code, E0110/E0211 help hints,
`List<T>`/`Option<T>` instance method dispatch (eliminates E0500 cascades).

| language  | generator | pass | check_fail | exec_fail | compile_fail | generator_fail | harness_error | skipped | total | pass% |
| --------- | --------- | ---- | ---------- | --------- | ------------ | -------------- | ------------- | ------- | ----- | ----- |
| tyra+spec | claude    | 98   | 0          | 0         | 2            | 0              | 0             | 0       | 100   | 98.0% |

**Run 16** (seed=1, 2026-05-28, **E0204 hard-error 前**): pass=91, compile_fail=9, 91.0%

**Note on Claude variance**: `--seed 1` doesn't fully determine
Claude CLI output, so each sweep samples slightly different code
per prompt. Comparing Run 14 vs Run 15 directly shows 17
regressions and 11 new passes — net pass -6, but the underlying
compiler is strictly stronger. For a stable reading, average Runs
14–15 → ~80 pass, or run multi-seed averaging on key milestones.

Historical passes on 100-prompt × tyra+spec × claude sweeps:

| Run | date       | pass | compile_pass | notable change                              |
| --- | ---------- | ---- | ------------ | ------------------------------------------- |
| 5   | 2026-04-21 | 16   | 16           | baseline spec injection                     |
| 6   | 2026-04-21 | 14   | 15           | + io stdlib + TYRA_STDLIB                   |
| 7   | 2026-04-21 | 26   | 33           | + stdlib source in context                  |
| 8   | 2026-04-21 | 25   | 40           | + anti-hallucination guide                  |
| 9   | 2026-04-22 | 32   | 40           | + `string` stdlib v0.1 (§17.3.4)            |
| 10  | 2026-04-23 | 58   | 76           | + Track B E0500 fixes + recursive ADT       |
| 11  | 2026-04-23 | 65   | 72           | + string extension + list stdlib + 066 fix  |
| 12  | 2026-04-23 | 66   | 79           | + Ty::Var compat (empty ListLit E0308)      |
| 13  | 2026-04-23 | 78   | 84           | + List<T> propagation + list.len / list.get |
| 14  | 2026-04-23 | 83   | 91           | + if/else arm-type unification (E0305)      |
| 15  | 2026-04-23 | 77   | 89           | + parser value/data/type keyword relaxation (variance -6 vs Run 14) |
| 16  | 2026-05-28 | 91   | 91           | v0.7.0: E0308 diag improvements + HAMT Map/Set + iteration + E0313 (E0204 hard-error 前) |
| 17  | 2026-05-28 | 98   | 98           | v0.7.0 post-release: E0204 hard error + E0213 + E0110/E0211 help + List/Option method dispatch |

Run 17 failing-prompt distribution (2 prompts, prompt-level):

| prompt              | error | actual cause                                                                 |
| ------------------- | ----- | ---------------------------------------------------------------------------- |
| 017-key-value-lookup | E0500 | Ty::Error cascade → `i64` vs `Option__Int` type mismatch in LLVM IR (residual codegen edge case) |
| 088-histogram        | E0100 | AI wrote `_ value: Int` — invalid labeled-parameter syntax (parser error)   |

Run 16 → Run 17 deltas (hardening 後の最終測定値; Run 16=seed=1, Run 17=seed=2 — cross-seed 比較のため因果帰属は参考値):

- **pass: 91 → 98 (+7, +8%)**
- **compile_fail: 9 → 2** — E0204 hard error converted `string.get` hallucinations from silent pass/exec_fail to proper compile_fail
- E0308: 0 (unchanged)
- E0500: 1 → 1 (residual — different prompt than Run 16; `017-key-value-lookup`)
- BUG/E0213: 2 → 0 — not observed in Run 17 (seed=2)
- E0110: 2 → 0 — not observed in Run 17 (seed=2)
- E0211: 1 → 0 — not observed in Run 17 (seed=2)
- E0204: 2 → 0 — not observed in Run 17 (seed=2)
- E0005: 1 → 0 — not observed in Run 17 (seed=2; variance)
- New E0100: 1 — AI syntax error (`_ value: Int`); language-model variance, not a compiler regression

The **2% residual** is attributable to one structural codegen bug (E0500, Ty::Error cascade in a specific pattern) and one AI syntax error. The structural bug is the next hardening target.

---

Run 11 error distribution (100 prompts):

| code  | count | typical cause                                         |
| ----- | ----- | ----------------------------------------------------- |
| E0308 | 50    | type mismatch (AI code doesn't match Tyra's strict typing) |
| E0305 | 14    | — (type-checker diagnostic)                           |
| E0104 | 5     | parser: reserved word / unexpected token              |
| E0200 | 3     | undefined name (2× `string` module-as-expr, 1× fabricated intrinsic) |
| E0102 | 2     | parser                                                |
| E0100 | 2     | parser                                                |
| E0500 | 1     | LLVM codegen (single edge-case)                       |

Run 10 → Run 11 deltas:

- E0500 4 → 1 (the 066 void-recursive fix plus the list stdlib
  closing `__list_int_*` hallucinations landed).
- E0200 14 → 3 (string extension + list stdlib fully absorbed the
  hallucination cluster; residual is 2 `string` module-level
  misuses and 1 fabricated intrinsic).
- E0104 14 → 5 (parser pressure eased as programs diverge from
  the ambiguous reserved-word zones).
- E0308 9 → 50 (type-checker is the new frontier — programs that
  previously failed earlier now reach type check and surface real
  mismatches).

Pass increased 58 → 65 (+7, +12%). compile_pass dropped slightly
(76 → 72, −4) but the programs that DO compile are now producing
correct output more reliably (pass / compile_pass ratio: 76% → 90%).

The failure mode has completely shifted: codegen and stdlib
hallucinations are essentially resolved. The dominant barrier is
now Tyra's type-checker rejecting the types AI writes naturally.
Relaxing the type-checker (or improving its diagnostics so AI
understands what to fix on retry) is the next attack surface.

Run 16 failing-prompt distribution (9 prompts, prompt-level):

| prompt              | error     | actual cause                                              |
| ------------------- | --------- | --------------------------------------------------------- |
| 010-count-vowels    | E0204     | hallucinated `string.get` — no such method in stdlib      |
| 026-count-lines     | BUG       | `fn main` + top-level statements both present             |
| 043-starts-with     | E0110     | `import` inside function body (must be top-level)         |
| 049-count-chars     | E0110     | `import` inside function body (must be top-level)         |
| 062-sum-two-squares | E0211     | `?` used in top-level statements (only valid in fn body)  |
| 076-running-max     | E0005     | integer literal overflows `Int` (i64); also E0104         |
| 090-balanced-parens | E0204     | hallucinated `string.get` — no such method in stdlib      |
| 096-rate-limit      | BUG       | `fn main` + top-level statements both present             |
| 099-sum-column      | E0500     | LLVM codegen: type mismatch in emitted IR (`i64` vs `ptr`) |

E0308: **0 occurrences** across all 100 prompts (was 50 occurrences in Run 11).

Run 15 → Run 16 deltas (v0.7.0 impact):

- **pass: 77 → 91 (+14, +18%)**
- **check_fail: 12 → 0** (all prompts that previously produced check_fail
  now compile and execute successfully; the harness measures single-shot
  pass/fail without retry, so this shows the compiler accepted more
  programs — not that diagnostics improved retry success)
- compile_fail: 11 → 9 (minor reduction)
- E0308: **50 occurrences (Run 11) → 0 (Run 16)** — the primary target
  of v0.7.0 diagnostic work is no longer observed in generated code

The residual failure surface for v0.8.0 prioritization:
- **Hallucinated stdlib methods** (`string.get`, 2 prompts): AI invents
  string methods not in stdlib v0.1; spec or error message needs to make
  available methods clearer.
- **`fn main` + top-level statements conflict** (BUG, 2 prompts): AI mixes
  top-level expressions with `fn main`; ADR-0006 Rule 2 needs better
  enforcement or clearer diagnostics.
- **`import` inside function body** (E0110, 2 prompts): AI places imports
  inside functions; parser/checker message may need to reinforce top-level
  restriction.
- **`?` in top-level statements** (E0211, 1 prompt): AI uses error
  propagation outside a function context.
- **Integer overflow literal** (E0005, 1 prompt): AI wrote `-9223372036854775808`
  directly; the parser sees the positive literal `9223372036854775808` which
  overflows `Int` (i64) before the unary minus is applied.
- **LLVM codegen IR type mismatch** (E0500, 1 prompt): pre-existing edge case.

`pass%` is computed against non-skipped runs so a missing compiler
does not depress the headline number.
