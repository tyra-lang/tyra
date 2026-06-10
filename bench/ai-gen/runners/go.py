from __future__ import annotations

import shutil
from pathlib import Path

from .base import Runner, StageResult


class GoRunner(Runner):
    name = "go"
    source_ext = ".go"

    def compiler_available(self) -> bool:
        return shutil.which("go") is not None

    def compile(self, workdir: Path) -> StageResult:
        src = workdir / "main.go"
        bin_path = workdir / "main"
        # Single-file `go build main.go` needs no go.mod. GOCACHE is kept
        # inside the throwaway workdir so benchmark runs leave no state
        # behind and cannot reuse a warm cache across prompts.
        return Runner.run_cmd(
            ["go", "build", "-o", str(bin_path), str(src)],
            cwd=workdir,
            timeout_s=self.config["timeouts"]["compile_seconds"],
            env={"GOCACHE": str(workdir / ".gocache")},
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
