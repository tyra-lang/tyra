# ai-gen benchmark summary

Prompts observed: 100

| language | generator | pass | check_fail | exec_fail | compile_fail | generator_fail | harness_error | skipped | total | pass% |
| -------- | --------- | ---- | ---------- | --------- | ------------ | -------------- | ------------- | ------- | ----- | ----- |
| crystal | claude | 96 | 1 | 0 | 3 | 0 | 0 | 0 | 100 | 96.0% |
| crystal | codex | 10 | 0 | 0 | 2 | 28 | 0 | 0 | 40 | 25.0% |
| gleam | claude | 37 | 0 | 6 | 57 | 0 | 0 | 0 | 100 | 37.0% |
| gleam | codex | 6 | 0 | 0 | 5 | 29 | 0 | 0 | 40 | 15.0% |
| ruby | claude | 99 | 1 | 0 | 0 | 0 | 0 | 0 | 100 | 99.0% |
| ruby | codex | 20 | 0 | 1 | 0 | 28 | 0 | 0 | 49 | 40.8% |
| tyra | claude | 0 | 0 | 0 | 100 | 0 | 0 | 0 | 100 | 0.0% |
| tyra | codex | 3 | 2 | 0 | 13 | 31 | 0 | 0 | 49 | 6.1% |
| v | claude | 49 | 1 | 0 | 50 | 0 | 0 | 0 | 100 | 49.0% |
| v | codex | 8 | 1 | 0 | 3 | 28 | 0 | 0 | 40 | 20.0% |

`pass%` is computed against non-skipped runs so a missing compiler does not depress the headline number.

