from __future__ import annotations

import subprocess
import time

from .base import Generator, GenerateResult


class ClaudeGenerator(Generator):
    """Wraps the local `claude` Code CLI (anthropic/claude-code).

    Invokes `claude -p --system-prompt ...` as a subprocess from a
    neutral cwd (/tmp) so per-project CLAUDE.md does not leak into
    the model's context. Uses the user's existing Claude Code auth
    (keychain / OAuth or ANTHROPIC_API_KEY — whichever the CLI is
    configured to use). The harness records the CLI version string
    for traceability; the actual model is whatever `claude` selects.
    """

    name = "claude"

    def __init__(self, binary: str = "claude", model: str | None = None):
        self.binary = binary
        self.model = model  # If None, Claude Code picks its configured default.
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
            self._version_cache = (out.stdout or out.stderr).strip().splitlines()[0]
        except Exception:
            self._version_cache = "claude (version probe failed)"
        return self._version_cache

    def generate(
        self,
        prompt_description: str,
        language: str,
        inject_tyra_spec: bool = False,
    ) -> GenerateResult:
        t0 = time.time()
        system = Generator.system_prompt(language, inject_tyra_spec=inject_tyra_spec)
        # `--disallowedTools` takes space-separated tool names in the
        # same argv slot (each as its own argv element). Harden: refuse
        # every interactive tool so the model has no way to do anything
        # other than emit text.
        disallowed = [
            "Bash", "Edit", "Write", "Read", "Glob", "Grep",
            "WebFetch", "WebSearch", "Agent", "NotebookEdit",
            "Skill", "ToolSearch", "TodoWrite", "ExitPlanMode",
            "AskUserQuestion",
        ]
        # NOTE: commander.js variadic args (`<tools...>`) run until the
        # next --flag or end-of-argv. --disallowedTools MUST be followed
        # by another --flag, otherwise it swallows the positional prompt.
        cmd = [
            self.binary,
            "-p",
            "--output-format", "text",
            "--exclude-dynamic-system-prompt-sections",
            "--disallowedTools", *disallowed,
            "--system-prompt", system,
        ]
        if self.model:
            cmd.extend(["--model", self.model])
        cmd.append(prompt_description)

        try:
            proc = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=180,
                cwd="/tmp",  # Neutral cwd so project CLAUDE.md is not loaded.
            )
            raw = proc.stdout or ""
            code = Generator.strip_fences(raw)
            dur = int((time.time() - t0) * 1000)
            if proc.returncode != 0 and not code.strip():
                return GenerateResult(
                    ok=False,
                    code="",
                    raw=raw,
                    model=self._version(),
                    duration_ms=dur,
                    error=f"claude exit {proc.returncode}: "
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
        except subprocess.TimeoutExpired as e:
            dur = int((time.time() - t0) * 1000)
            return GenerateResult(
                ok=False,
                code="",
                raw="",
                model=self._version(),
                duration_ms=dur,
                error=f"TimeoutExpired after {e.timeout}s",
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
