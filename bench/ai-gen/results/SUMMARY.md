# ai-gen benchmark summary

Prompts observed: 39

| language | generator | pass | check_fail | exec_fail | compile_fail | generator_fail | harness_error | skipped | total | pass% |
| -------- | --------- | ---- | ---------- | --------- | ------------ | -------------- | ------------- | ------- | ----- | ----- |
| crystal | codex | 10 | 0 | 0 | 2 | 18 | 0 | 0 | 30 | 33.3% |
| gleam | codex | 6 | 0 | 0 | 5 | 19 | 0 | 0 | 30 | 20.0% |
| ruby | codex | 20 | 0 | 1 | 0 | 18 | 0 | 0 | 39 | 51.3% |
| tyra | codex | 3 | 2 | 0 | 13 | 21 | 0 | 0 | 39 | 7.7% |
| v | codex | 8 | 1 | 0 | 3 | 18 | 0 | 0 | 30 | 26.7% |

`pass%` is computed against non-skipped runs so a missing compiler does not depress the headline number.

