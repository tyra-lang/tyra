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
    """Return the Tyra spec + all example programs as a single string.

    Cached on the module so we pay the disk read once per process.
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
    context = (
        "# Tyra language spec\n\n"
        f"{spec}\n\n"
        "# Example programs\n\n"
        "The following programs are canonical Tyra examples. Use them "
        "as a syntax reference when generating code.\n\n"
        + "\n\n".join(example_blocks)
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
