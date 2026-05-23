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
# time: 0.012s

2 passed, 0 failed
```

Exit code is 0 when all tests pass, 1 when any test fails.

---

## Filtering tests

Run only tests whose names contain a substring:

```bash
tyra test --filter add
```

This runs `test_addition` but skips `test_subtraction`. Useful for focusing on
a single area without running the whole suite.

To list which tests would run without actually running them:

```bash
tyra test --list
tyra test --filter add --list
```

Output order is stable: files in lexicographic path order, functions in
source-declaration order within each file.

---

## Parallel execution and timeouts

Run tests in parallel across multiple workers:

```bash
tyra test --jobs 4
```

The default is 1 (sequential). Output order is deterministic regardless of
completion order.

Set a per-test wall-clock limit:

```bash
tyra test --timeout 10
```

A test that exceeds the limit is killed and counted as a failure. Combine with
`--jobs` for fast, bounded CI runs:

```bash
tyra test --jobs 4 --timeout 10
```

---

## Per-test process isolation

Each `test_*` function runs in its own subprocess (v0.5.0+). A panic, abort, or
out-of-memory event in one test does not prevent sibling tests from running or
appearing in the output. The TAP output format is unchanged.

---

## JUnit XML output

For CI systems that consume JUnit XML (Jenkins, GitHub Actions test summary, etc.):

```bash
tyra test --format junit
```

Output is a JUnit-compatible XML document:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<testsuites>
  <testsuite name="math_test" tests="2" failures="0" time="0.012">
    <testcase name="test_addition" classname="math_test"/>
    <testcase name="test_subtraction" classname="math_test"/>
  </testsuite>
</testsuites>
```

When a test fails, the failing test case includes a `<failure>` element:

```xml
<testcase name="test_bad" classname="math_test">
  <failure message="expected 2, got 3"/>
</testcase>
```

If the test file cannot be compiled (an infrastructure failure), the runner
emits a synthetic single-test suite with the compile error as the failure
message, so CI always sees a concrete failure rather than a silent zero-test
result.

Combine with `--filter` to scope the report:

```bash
tyra test --filter add --format junit
```

**GitHub Actions integration example:**

```yaml
- name: Run tests
  run: TYRA_STDLIB=$PWD/stdlib tyra test --format junit src/ > test-results.xml || true

- name: Publish test results
  uses: mikepenz/action-junit-report@v4
  if: always()
  with:
    report_paths: test-results.xml
```

The `|| true` prevents the step from failing before the report action uploads
results. The report action marks the check as failed when `<failure>` elements
are present.

---

## Assertion helpers

Import `assert` to get typed assertion functions:

| Function | Checks |
|---|---|
| `assert.eq(a, b)` | two values of the same type are equal (`Int`, `String`, or `Bool`) |
| `assert.eq_str(a, b)` | two `String` values are equal (explicit typed form) |
| `assert.eq_bool(a, b)` | two `Bool` values are equal (explicit typed form) |
| `assert.ne(a, b)` | two values of the same type differ (`Int`, `String`, or `Bool`) |
| `assert.ne_str(a, b)` | two `String` values differ (explicit typed form) |
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
