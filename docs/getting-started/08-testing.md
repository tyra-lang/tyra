# 8. Testing Your Code

Tyra includes a built-in test runner. No third-party framework is required.

---

## Writing tests

Create a file named `*_test.tyra` (for example `math_test.tyra`) alongside your
source files. Inside, write functions whose names start with `test_` and return
`Result<Unit, String>`:

```tyra
import assert

fn test_addition() -> Result<Unit, String>
  assert.eq(1 + 1, 2)?
  assert.eq(10 + 5, 15)?
  Ok(())
end

fn test_subtraction() -> Result<Unit, String>
  assert.eq(10 - 3, 7)?
  Ok(())
end
```

Use `?` to propagate a failure immediately. A test passes when it returns
`Ok(())` and fails when it returns `Err(message)`.

---

## Running tests

```bash
# Run all *_test.tyra files in the current directory (recursive)
tyra test

# Run tests in a specific directory
tyra test src/

# Run a single test file
tyra test math_test.tyra
```

Output follows the TAP (Test Anything Protocol) format:

```
# math_test.tyra
TAP version 14
1..2
ok 1 - test_addition
ok 2 - test_subtraction

2 passed, 0 failed
```

Exit code is 0 when all tests pass, 1 when any test fails.

---

## Assertion helpers

Import `assert` to get typed assertion functions:

| Function | Checks |
|---|---|
| `assert.eq(a, b)` | two `Int` values are equal |
| `assert.eq_str(a, b)` | two `String` values are equal |
| `assert.eq_bool(a, b)` | two `Bool` values are equal |
| `assert.ne(a, b)` | two `Int` values differ |
| `assert.ne_str(a, b)` | two `String` values differ |
| `assert.is_ok(result)` | a `Result` is `Ok` |
| `assert.is_err(result)` | a `Result` is `Err` |

All helpers return `Result<Unit, String>`. Use `?` to propagate the failure and
stop the test immediately. If you do not use `?`, the return value is discarded
and the test continues — but you must explicitly return `Err(...)` yourself for
the runner to count it as a failure. A test that ends with `Ok(())` always
passes, regardless of any ignored assertion results.

---

## Rules for test files

- The file name must end with `_test.tyra`
- Test functions must have no parameters and return `Result<Unit, String>`
- Test files must not contain `fn main` or top-level executable statements

---

## Next steps

- Explore the [language specification](../spec/ja/language-spec.md) for the full
  `assert` API reference (§17, stdlib)
- See [ADR-0008](../design/0008-test-runner.md) for the test runner design rationale
