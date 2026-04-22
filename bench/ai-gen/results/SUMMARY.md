# ai-gen benchmark summary

Prompts observed: 100

Latest sweep: **Run 10** (`tyra+spec × claude × 100`, 2026-04-23)
after the Track B E0500 cluster fixes and recursive ADT support.

| language  | generator | pass | check_fail | exec_fail | compile_fail | generator_fail | harness_error | skipped | total | pass% |
| --------- | --------- | ---- | ---------- | --------- | ------------ | -------------- | ------------- | ------- | ----- | ----- |
| tyra+spec | claude    | 58   | 17         | 1         | 19           | 5              | 0             | 0       | 100   | 58.0% |

Historical passes on 100-prompt × tyra+spec × claude sweeps:

| Run | date       | pass | compile_pass | notable change                              |
| --- | ---------- | ---- | ------------ | ------------------------------------------- |
| 5   | 2026-04-21 | 16   | 16           | baseline spec injection                     |
| 6   | 2026-04-21 | 14   | 15           | + io stdlib + TYRA_STDLIB                   |
| 7   | 2026-04-21 | 26   | 33           | + stdlib source in context                  |
| 8   | 2026-04-21 | 25   | 40           | + anti-hallucination guide                  |
| 9   | 2026-04-22 | 32   | 40           | + `string` stdlib v0.1 (§17.3.4)            |
| 10  | 2026-04-23 | 58   | 76           | + Track B E0500 fixes + recursive ADT      |

Run 10 error distribution (100 prompts):

| code  | count | typical cause                                |
| ----- | ----- | -------------------------------------------- |
| E0104 | 14    | parser: reserved word / unexpected token     |
| E0200 | 14    | fabricated intrinsic / undefined name        |
| E0308 | 9     | type mismatch                                |
| E0101 | 7     | parser: expected newline / EOF               |
| E0500 | 4     | LLVM codegen (mostly void-recursive 066-class) |
| E0103 | 3     | parser                                       |
| E0305 | 2     | —                                            |
| E0102 | 1     | parser                                       |
| E0100 | 1     | parser                                       |

Run 9 → Run 10 deltas (driven by Track B + recursive ADT):

- E0500 28 → 4 (-24): recursive ADT, pattern/let hoist, if-arm
  bare-Ident, Unit-assignment tail, struct/Option field type hints,
  `.copy()` inference — all landed.
- E0104 10 → 14 (+4): variance across Claude sweeps.
- E0200 8 → 14 (+6): with more programs reaching further, new
  hallucinations surface (mostly missing stdlib surface).
- E0308 2 → 9 (+7): same — more programs now type-check far enough
  to hit genuine type mismatches.
- E0101 0 → 7 (+7): parser tail edge cases in newly reaching
  programs.

Pass increased 32 → 58 (+26, +81%). compile_pass increased 40 → 76
(+36, +90%). The compile-surviving / correctness gap is now
dominated by `E0308` (type-checker strictness against AI-generated
code) and the residual stdlib hallucination cluster, not by
codegen bugs.

`pass%` is computed against non-skipped runs so a missing compiler
does not depress the headline number.
