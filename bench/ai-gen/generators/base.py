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
        "there is no `string`, `collections`, or `core.io` module. "
        "Call functions exactly as they are exported below.\n\n"
        + "\n\n".join(stdlib_blocks)
        + "\n\n# v0.1 method hallucinations to avoid\n\n"
        "The following methods DO NOT exist in Tyra v0.1 — using them "
        "generates invalid LLVM IR. If you catch yourself reaching for "
        "one, rewrite with the listed alternative:\n\n"
        "- `opt.unwrap_or(default)` — use `match opt when Some(x) x "
        "when None default end` instead.\n"
        "- `opt.ok_or(err)` / `opt.ok_or_else(...)` — use `match opt "
        "when Some(x) Ok(x) when None Err(err) end`.\n"
        "- `result.unwrap_or(default)` — use `match result when Ok(x) "
        "x when Err(_) default end`.\n"
        "- `str.trim()` / `str.split(...)` / `str.split_whitespace()` / "
        "`str.chars()` / `str.runes()` / `str.len()` — there is no "
        "String method API in v0.1. For stdin parsing, use `io.read_line()` "
        "then operate on the raw String using string interpolation or "
        "character-by-character iteration in pattern matches.\n"
        "- `int.to_string()` — use interpolation `\"#{n}\"`.\n"
        "- The `?` operator works ONLY on `Option<T>` / `Result<T, E>` "
        "return positions; do not chain it after arbitrary calls.\n"
        "- `for x in 1..=n` — use `while` with a counter; Tyra v0.1 does "
        "not have numeric ranges as values.\n"
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
