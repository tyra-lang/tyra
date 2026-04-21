from __future__ import annotations

import shutil
from pathlib import Path

from .base import Runner, StageResult


class RubyRunner(Runner):
    """Ruby has no AOT compile step. Use `ruby -c` as a syntax gate.

    This is a weaker check than a type-checker — it only catches parse
    errors — but it is the closest analogue available in the stock
    ruby toolchain and keeps the stage schema uniform across languages.
    """

    name = "ruby"
    source_ext = ".rb"

    def compiler_available(self) -> bool:
        return shutil.which("ruby") is not None

    def compile(self, workdir: Path) -> StageResult:
        src = workdir / "main.rb"
        return Runner.run_cmd(
            ["ruby", "-c", str(src)],
            cwd=workdir,
            timeout_s=self.config["timeouts"]["compile_seconds"],
        )

    def execute(self, workdir: Path, stdin: str, timeout_s: int) -> StageResult:
        src = workdir / "main.rb"
        return Runner.run_cmd(
            ["ruby", str(src)],
            cwd=workdir,
            stdin=stdin,
            timeout_s=timeout_s,
        )
