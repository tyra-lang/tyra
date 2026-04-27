from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


@dataclass
class GenerateResult:
    ok: bool
    code: str
    raw: str
    model: str
    duration_ms: int
    error: Optional[str] = None


# Repo root. bench/ai-gen/generators/base.py -> four parents back.
_REPO_ROOT = Path(__file__).resolve().parents[3]


def _load_tyra_context() -> str:
    """Return the Tyra spec + examples + stdlib modules as one blob.

    Cached on the module so we pay the disk read once per process.

    Previous iterations of this function included only the spec and
    example programs. That left the model guessing at stdlib APIs —
    it would reach for `import string` (which does not exist) or call
    `.trim()` on String (which is not provided). Including the stdlib
    source gives the model the authoritative module list and exact
    function signatures, cutting out hallucinated imports.
    """
    cached = getattr(_load_tyra_context, "_cache", None)
    if cached is not None:
        return cached
    spec = (_REPO_ROOT / "docs" / "spec" / "en" / "language-spec.md").read_text()
    examples_dir = _REPO_ROOT / "examples"
    example_blocks: list[str] = []
    for path in sorted(examples_dir.glob("*.tyra")):
        example_blocks.append(
            f"### {path.name}\n\n```tyra\n{path.read_text()}\n```"
        )
    stdlib_dir = _REPO_ROOT / "stdlib"
    stdlib_blocks: list[str] = []
    # Recurse so http/client.tyra and http/server.tyra are picked up too.
    for path in sorted(stdlib_dir.rglob("*.tyra")):
        rel = path.relative_to(stdlib_dir)
        stdlib_blocks.append(
            f"### stdlib/{rel}\n\n```tyra\n{path.read_text()}\n```"
        )
    context = (
        "# Tyra language spec\n\n"
        f"{spec}\n\n"
        "# Example programs\n\n"
        "The following programs are canonical Tyra examples. Use them "
        "as a syntax reference when generating code.\n\n"
        + "\n\n".join(example_blocks)
        + "\n\n# Standard library (authoritative module list)\n\n"
        "These are the ONLY stdlib modules available in Tyra v0.1. "
        "Do not import modules that are not listed here — in particular "
        "there is no `collections` or `core.io` module. "
        "Call functions exactly as they are exported below.\n\n"
        + "\n\n".join(stdlib_blocks)
        + "\n\n# v0.1 method reference\n\n"
        "Available primitive methods in v0.1 are LIMITED. Stick to the "
        "list below; anything else produces invalid LLVM IR.\n\n"
        "**Valid methods:**\n"
        "- `list.len()` — List<T> length as Int\n"
        "- `list.get(i)` — List<T> indexed access returning Option<T>\n"
        "- `opt.ok_or(err)` — converts Option<T> → Result<T, E>\n"
        "- `value.copy()` — explicit copy of a `value` type (§8.5)\n"
        "- `x.to_string()` — only when `impl Stringable for T` exists;\n"
        "  primitives use string interpolation `\"#{x}\"` instead.\n"
        "- `task.await` — on a `spawn`-produced task handle.\n\n"
        "**Methods that DO NOT exist — rewrite as shown:**\n"
        "- `opt.unwrap_or(default)` → `match opt when Some(x) x when None default end`\n"
        "- `result.unwrap_or(default)` → `match result when Ok(x) x when Err(_) default end`\n"
        "- `str.trim()` / `str.len()` / `str.contains(...)` — there are no "
        "String *methods* in v0.1. Use the `string` module instead: "
        "`import string` then call `string.trim(s)`, `string.len(s)`, "
        "`string.contains(s, needle)`, `string.starts_with(s, p)`, "
        "`string.ends_with(s, p)`, `string.to_upper(s)`, `string.to_lower(s)`, "
        "`string.is_empty(s)`, `string.parse_int(s)` (returns Option<Int>). "
        "Tokenizing helpers: `string.split_whitespace(s)` returns "
        "`List<String>` of non-whitespace runs (collapses adjacent ws), "
        "and `string.split(s, sep)` returns `List<String>` split on every "
        "byte-level occurrence of `sep` (empty `sep` returns `[s]`). "
        "**Important:** in v0.1, `list.len(xs)`, `list.get(xs, i)`, and "
        "`list.push(xs, x)` work polymorphically for `List<String>` too "
        "(the compiler redirects them to element-type-agnostic builtins). "
        "However `list.sum(xs)` / `list.contains(xs, x)` / "
        "`list.index_of(xs, x)` / `list.max(xs)` / `list.min(xs)` are "
        "still `List<Int>` ONLY — for `List<String>` you must iterate "
        "manually with `while i < xs.len()` and `xs.get(i)`. Method-call "
        "form `xs.len()` / `xs.get(i)` always works regardless of element "
        "type.\n"
        "Note: `string.replace(...)` / `string.join(...)` / `str.chars()` "
        "/ `str.runes()` are NOT available in v0.1.\n"
        "**Map<String, Int>** is supported in v0.1: literal "
        "`let m: Map<String, Int> = {\"k\": 1, ...}`, `m.get(k) -> "
        "Option<Int>`, `m.contains_key(k) -> Bool`. Other K / V "
        "combinations are NOT supported (no Map<String, String> etc.). "
        "There is no `m.put(k, v)` or iteration in v0.1 — Maps are "
        "constructed once via the literal and read-only after. To "
        "\"update\", build a new literal.\n"
        "- `int.to_string()` — use interpolation `\"#{n}\"`.\n"
        "- `list.map(...)` / `list.filter(...)` / `list.fold(...)` — use "
        "`for x in list` with a `mut` accumulator.\n\n"
        "**Imports are mandatory — Tyra has no auto-imports.** "
        "Whenever your code calls `string.xxx(...)`, `io.xxx(...)`, "
        "`list.xxx(...)`, etc., the file MUST start with the matching "
        "`import string` / `import io` / `import list` line. Calling a "
        "module function without its import produces `E0200: undefined "
        "name` and the program fails to compile. When in doubt, add the "
        "import — unused imports are harmless.\n\n"
        "**Other gotchas:**\n"
        "- `?` works ONLY on `Option<T>` / `Result<T, E>` inside a "
        "function whose return type is compatible. Do not chain it after "
        "arbitrary calls. **`?` is NOT allowed at the top level** — "
        "Tyra desugars top-level code to `fn main() -> Unit`, which "
        "cannot propagate errors. If your program needs `?`, write an "
        "explicit `fn main() -> Result<Unit, String>` (or `Option<Unit>`) "
        "and put your logic inside it, ending with `Ok(())`. Top-level "
        "`return`, `.await`, and `?` all produce E0210 / E0211 / E0212.\n"
        "- `for x in 1..=n` — no numeric range literal in v0.1. Use "
        "`while` with a counter.\n"
        "- There is no `%` on Float, only on Int.\n"
        "- **String concatenation via `+` does NOT exist**. `s1 + s2` is "
        "invalid and produces invalid LLVM IR. Always use interpolation: "
        "`\"#{s1}#{s2}\"`. To append a single byte, use "
        "`result = \"#{result}#{string.from_byte(b)}\"`.\n"
        "- **No `++` operator**. There is no list/string concatenation "
        "operator in v0.1. To grow a list, use `list.push(xs, x)` "
        "(works for `List<Int>` and `List<String>`); for strings use "
        "interpolation as above.\n"
        "- **No bitwise operators (`^`, `&`, `|`, `<<`, `>>`)**. Tyra "
        "v0.1 has no bitwise ops. To toggle ASCII case use arithmetic: "
        "`if b >= 65 and b <= 90 then b + 32 else if b >= 97 and "
        "b <= 122 then b - 32 else b`.\n"
        "- **No `.unwrap()` / `.unwrap_value()` methods**. Always pattern "
        "match: `match opt when Some(x) x when None default end`. "
        "`Option`/`Result` have NO unwrap-style methods in v0.1.\n"
    )
    _load_tyra_context._cache = context  # type: ignore[attr-defined]
    return context


class Generator:
    """Abstract base. Subclasses implement `generate`."""

    name: str = "base"

    def generate(
        self,
        prompt_description: str,
        language: str,
        inject_tyra_spec: bool = False,
    ) -> GenerateResult:
        raise NotImplementedError

    @staticmethod
    def system_prompt(language: str, inject_tyra_spec: bool = False) -> str:
        # Baseline prompt is identical across generators so comparisons
        # measure the model, not the prompt. When inject_tyra_spec and
        # the target is Tyra, the full spec + example programs are
        # appended so the model can reference them. This is the
        # controlled experiment for strategy.md §4.1.
        base = (
            f"You are writing a small {language} program. "
            "Respond with ONLY the source code, no markdown fences, "
            "no commentary, no explanations. Read input from stdin "
            "and write output to stdout when the task requires it. "
            "The program must compile and run as-is with the default "
            f"{language} toolchain."
        )
        if inject_tyra_spec and language == "tyra":
            base = base + "\n\n" + _load_tyra_context()
        return base

    @staticmethod
    def strip_fences(text: str) -> str:
        # Models often ignore "no markdown fences" and wrap anyway. Strip
        # a single enclosing fenced block; leave everything else alone.
        m = re.match(r"^\s*```[a-zA-Z0-9_+-]*\n(.*?)\n```\s*$", text, re.DOTALL)
        if m:
            return m.group(1)
        return text.strip()
