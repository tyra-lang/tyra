# ai-gen benchmark summary

Prompts observed: 100

Latest sweep: **Run 12** (`tyra+spec × claude × 100`, 2026-04-23)
after the Ty::Var structural-compatibility fix (empty ListLit
E0308 cluster).

| language  | generator | pass | check_fail | exec_fail | compile_fail | generator_fail | harness_error | skipped | total | pass% |
| --------- | --------- | ---- | ---------- | --------- | ------------ | -------------- | ------------- | ------- | ----- | ----- |
| tyra+spec | claude    | 66   | 6          | 7         | 21           | 0              | 0             | 0       | 100   | 66.0% |

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

`pass%` is computed against non-skipped runs so a missing compiler
does not depress the headline number.
