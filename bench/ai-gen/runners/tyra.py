from __future__ import annotations

import shutil
from pathlib import Path

from .base import Runner, StageResult


class TyraRunner(Runner):
    name = "tyra"
    source_ext = ".tyra"

    def compiler_available(self) -> bool:
        return (self.repo_root / "target" / "debug" / "tyra").exists()

    def _binary(self) -> Path:
        return self.repo_root / "target" / "debug" / "tyra"

    def compile(self, workdir: Path) -> StageResult:
        src = workdir / "main.tyra"
        # tyra build <src> currently emits `a.out` next to the source.
        # Use `tyra build` so type checking + codegen both run.
        # The workdir sits under /tmp so the compiler's default stdlib
        # walk-up search won't find the repo's stdlib/. Point at it
        # explicitly via TYRA_STDLIB so `import io` etc. resolve.
        res = Runner.run_cmd(
            [str(self._binary()), "build", str(src)],
            cwd=workdir,
            timeout_s=self.config["timeouts"]["compile_seconds"],
            env={"TYRA_STDLIB": str(self.repo_root / "stdlib")},
        )
        return res

    def execute(self, workdir: Path, stdin: str, timeout_s: int) -> StageResult:
        # `tyra build main.tyra` emits `main` (source stem, no extension).
        bin_path = workdir / "main"
        if not bin_path.exists():
            # Historical fallback in case CLI behavior changes.
            alt = workdir / "a.out"
            if alt.exists():
                bin_path = alt
            else:
                return StageResult(
                    ok=False,
                    duration_ms=0,
                    stderr=f"no compiled binary at {bin_path}",
                )
        return Runner.run_cmd(
            [str(bin_path)],
            cwd=workdir,
            stdin=stdin,
            timeout_s=timeout_s,
        )
