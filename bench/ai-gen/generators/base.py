from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Optional


@dataclass
class GenerateResult:
    ok: bool
    code: str
    raw: str
    model: str
    duration_ms: int
    error: Optional[str] = None


class Generator:
    """Abstract base. Subclasses implement `generate`."""

    name: str = "base"

    def generate(self, prompt_description: str, language: str) -> GenerateResult:
        raise NotImplementedError

    @staticmethod
    def system_prompt(language: str) -> str:
        # The system prompt is identical across generators so comparisons
        # measure the model, not the prompt. Language-specific hints are
        # deliberately minimal: no stdlib tips, no syntax crib — we want
        # to measure how well each language's idioms align with what the
        # model already knows.
        return (
            f"You are writing a small {language} program. "
            "Respond with ONLY the source code, no markdown fences, "
            "no commentary, no explanations. Read input from stdin "
            "and write output to stdout when the task requires it. "
            "The program must compile and run as-is with the default "
            f"{language} toolchain."
        )

    @staticmethod
    def strip_fences(text: str) -> str:
        # Models often ignore "no markdown fences" and wrap anyway. Strip
        # a single enclosing fenced block; leave everything else alone.
        m = re.match(r"^\s*```[a-zA-Z0-9_+-]*\n(.*?)\n```\s*$", text, re.DOTALL)
        if m:
            return m.group(1)
        return text.strip()
