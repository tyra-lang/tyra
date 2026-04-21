from __future__ import annotations

import os
import time
from typing import Optional

from .base import Generator, GenerateResult


class ClaudeGenerator(Generator):
    name = "claude"

    def __init__(
        self,
        model: str = "claude-opus-4-7",
        max_tokens: int = 2048,
        temperature: float = 0.2,
    ):
        self.model = model
        self.max_tokens = max_tokens
        self.temperature = temperature
        self._client = None

    def _client_or_raise(self):
        if self._client is not None:
            return self._client
        try:
            import anthropic  # type: ignore
        except ImportError as e:
            raise RuntimeError(
                "anthropic SDK not installed; pip install -r requirements.txt"
            ) from e
        api_key = os.environ.get("ANTHROPIC_API_KEY")
        if not api_key:
            raise RuntimeError("ANTHROPIC_API_KEY is not set")
        self._client = anthropic.Anthropic(api_key=api_key)
        return self._client

    def generate(self, prompt_description: str, language: str) -> GenerateResult:
        t0 = time.time()
        try:
            client = self._client_or_raise()
            resp = client.messages.create(
                model=self.model,
                max_tokens=self.max_tokens,
                temperature=self.temperature,
                system=Generator.system_prompt(language),
                messages=[{"role": "user", "content": prompt_description}],
            )
            raw = "".join(
                block.text
                for block in resp.content
                if getattr(block, "type", None) == "text"
            )
            code = Generator.strip_fences(raw)
            dur = int((time.time() - t0) * 1000)
            if not code.strip():
                return GenerateResult(
                    ok=False,
                    code="",
                    raw=raw,
                    model=self.model,
                    duration_ms=dur,
                    error="empty response",
                )
            return GenerateResult(
                ok=True, code=code, raw=raw, model=self.model, duration_ms=dur
            )
        except Exception as e:
            dur = int((time.time() - t0) * 1000)
            return GenerateResult(
                ok=False,
                code="",
                raw="",
                model=self.model,
                duration_ms=dur,
                error=f"{type(e).__name__}: {e}",
            )
