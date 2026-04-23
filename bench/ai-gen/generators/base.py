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
        "Note: `string.split(...)` / `string.split_whitespace()` / "
        "`string.replace(...)` / `string.join(...)` / `str.chars()` / "
        "`str.runes()` are NOT available in v0.1 — work byte-by-byte via "
        "interpolation or read stdin line-by-line with `io.read_line()`.\n"
        "- `int.to_string()` — use interpolation `\"#{n}\"`.\n"
        "- `list.map(...)` / `list.filter(...)` / `list.fold(...)` — use "
        "`for x in list` with a `mut` accumulator.\n\n"
        "**Other gotchas:**\n"
        "- `?` works ONLY on `Option<T>` / `Result<T, E>` inside a "
        "function whose return type is compatible. Do not chain it after "
        "arbitrary calls.\n"
        "- `for x in 1..=n` — no numeric range literal in v0.1. Use "
        "`while` with a counter.\n"
        "- There is no `%` on Float, only on Int.\n"
        "- **String concatenation via `+` does NOT exist**. `s1 + s2` is "
        "invalid and produces invalid LLVM IR. Always use interpolation: "
        "`\"#{s1}#{s2}\"`. To append a single byte, use "
        "`result = \"#{result}#{string.from_byte(b)}\"`.\n"
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
