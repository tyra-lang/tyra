from __future__ import annotations

import shutil
from pathlib import Path

from .base import Runner, StageResult


class VRunner(Runner):
    name = "v"
    source_ext = ".v"

    def compiler_available(self) -> bool:
        return shutil.which("v") is not None

    def compile(self, workdir: Path) -> StageResult:
        src = workdir / "main.v"
        bin_path = workdir / "main"
        return Runner.run_cmd(
            ["v", "-o", str(bin_path), str(src)],
            cwd=workdir,
            timeout_s=self.config["timeouts"]["compile_seconds"],
        )

    def execute(self, workdir: Path, stdin: str, timeout_s: int) -> StageResult:
        bin_path = workdir / "main"
        if not bin_path.exists():
            return StageResult(
                ok=False, duration_ms=0, stderr=f"no binary at {bin_path}"
            )
        return Runner.run_cmd(
            [str(bin_path)], cwd=workdir, stdin=stdin, timeout_s=timeout_s
        )
