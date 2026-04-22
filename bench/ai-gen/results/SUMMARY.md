# ai-gen benchmark summary

Prompts observed: 100

Latest sweep: **Run 9** (`tyra+spec × claude × 100`, 2026-04-22) after
shipping the v0.1 `string` stdlib (§17.3.4).

| language  | generator | pass | check_fail | exec_fail | compile_fail | generator_fail | harness_error | skipped | total | pass% |
| --------- | --------- | ---- | ---------- | --------- | ------------ | -------------- | ------------- | ------- | ----- | ----- |
| tyra+spec | claude    | 32   | 8          | 0         | 52           | 8              | 0             | 0       | 100   | 32.0% |

Historical passes on 100-prompt × tyra+spec × claude sweeps:

| Run | date       | pass | compile_pass | top failure cluster                  |
| --- | ---------- | ---- | ------------ | ------------------------------------ |
| 5   | 2026-04-21 | 16   | 16           | `import string` rejected             |
| 6   | 2026-04-21 | 14   | 15           | string method hallucinations         |
| 7   | 2026-04-21 | 26   | 33           | mixed E0500 / E0104                  |
| 8   | 2026-04-21 | 25   | 40           | E0104 (42) / E0200 (38) / E0101 (24) |
| 9   | 2026-04-22 | 32   | 40           | E0500 (28) / E0104 (10) / E0200 (8)  |

Run 9 error distribution (100 prompts):

| code  | count | typical cause                                 |
| ----- | ----- | --------------------------------------------- |
| E0500 | 28    | LLVM codegen type mismatch / recursive ADT    |
| E0104 | 10    | parser: reserved word used as identifier      |
| E0200 | 8     | fabricated intrinsic (5× `__string_byte_at`) |
| E0305 | 8     | —                                             |
| E0100 | 4     | parser                                        |
| E0304 | 4     | —                                             |
| E0308 | 2     | type mismatch                                 |

Run 8 → Run 9 deltas:

- E0104 42 → 10 (−32)
- E0200 38 → 8 (−30)
- E0101 24 → 0 (−24)
- E0308 15 → 2 (−13)
- E0500 22 → 28 (+6; more programs survive earlier phases and now hit
  the real codegen bugs)

`pass%` is computed against non-skipped runs so a missing compiler does
not depress the headline number.
