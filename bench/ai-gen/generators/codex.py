from __future__ import annotations

import os
import subprocess
import tempfile
import time
from typing import List

from .base import Generator, GenerateResult


class CodexGenerator(Generator):
    """Wraps the local `codex exec` CLI.

    The Codex CLI chooses its own model by default (whatever the user
    configured in `~/.codex/config.toml`). The harness records the
    CLI version so each result is traceable; we do not force a model.
    """

    name = "codex"

    def __init__(self, binary: str = "codex", extra_args: List[str] | None = None):
        self.binary = binary
        self.extra_args = list(extra_args or [])
        self._version_cache: str | None = None

    def _version(self) -> str:
        if self._version_cache is not None:
            return self._version_cache
        try:
            out = subprocess.run(
                [self.binary, "--version"],
                capture_output=True,
                text=True,
                timeout=10,
            )
            self._version_cache = (out.stdout or out.stderr).strip()
        except Exception:
            self._version_cache = "codex (version probe failed)"
        return self._version_cache

    def generate(
        self,
        prompt_description: str,
        language: str,
        inject_tyra_spec: bool = False,
    ) -> GenerateResult:
        t0 = time.time()
        system = Generator.system_prompt(language, inject_tyra_spec=inject_tyra_spec)
        full_prompt = f"{system}\n\n---\n\n{prompt_description}"
        with tempfile.NamedTemporaryFile(
            "r+", suffix=".txt", delete=False
        ) as tmp:
            last_msg_path = tmp.name

        try:
            cmd = [
                self.binary,
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "--color",
                "never",
                "-s",
                "read-only",
                "--output-last-message",
                last_msg_path,
                *self.extra_args,
                full_prompt,
            ]
            proc = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=180,
            )
            with open(last_msg_path, "r") as f:
                raw = f.read()
            code = Generator.strip_fences(raw)
            dur = int((time.time() - t0) * 1000)
            if proc.returncode != 0 and not code.strip():
                return GenerateResult(
                    ok=False,
                    code="",
                    raw=raw,
                    model=self._version(),
                    duration_ms=dur,
                    error=f"codex exec exit {proc.returncode}: "
                    f"{(proc.stderr or '')[-500:]}",
                )
            if not code.strip():
                return GenerateResult(
                    ok=False,
                    code="",
                    raw=raw,
                    model=self._version(),
                    duration_ms=dur,
                    error="empty response",
                )
            return GenerateResult(
                ok=True,
                code=code,
                raw=raw,
                model=self._version(),
                duration_ms=dur,
            )
        except Exception as e:
            dur = int((time.time() - t0) * 1000)
            return GenerateResult(
                ok=False,
                code="",
                raw="",
                model=self._version(),
                duration_ms=dur,
                error=f"{type(e).__name__}: {e}",
            )
        finally:
            try:
                os.unlink(last_msg_path)
            except OSError:
                pass
