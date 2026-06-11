from __future__ import annotations

import os
import shutil
from pathlib import Path

from .base import Runner, StageResult


class TyraRunner(Runner):
    name = "tyra"
    source_ext = ".ty"

    def _binary(self) -> Path:
        # TYRA_BIN lets external users point at their installed tyra.
        # Falls back to release then debug in-repo build.
        override = os.environ.get("TYRA_BIN")
        if override:
            return Path(override)
        release = self.repo_root / "target" / "release" / "tyra"
        if release.exists():
            return release
        return self.repo_root / "target" / "debug" / "tyra"

    def compiler_available(self) -> bool:
        override = os.environ.get("TYRA_BIN")
        if override:
            return Path(override).exists()
        return (
            (self.repo_root / "target" / "release" / "tyra").exists()
            or (self.repo_root / "target" / "debug" / "tyra").exists()
        )

    def _stdlib_dir(self) -> Path:
        # When TYRA_BIN points at an installed binary, the stdlib lives at
        # <prefix>/lib/tyra/stdlib/ (FHS layout from install.sh).
        # Fall back to the in-repo stdlib/ for development use.
        override = os.environ.get("TYRA_BIN")
        if override:
            bin_path = Path(override).resolve()
            stdlib = bin_path.parent.parent / "lib" / "tyra" / "stdlib"
            if stdlib.is_dir():
                return stdlib
        return self.repo_root / "stdlib"

    def compile(self, workdir: Path) -> StageResult:
        src = workdir / "main.ty"
        # The workdir sits under /tmp so the compiler's default stdlib
        # walk-up search won't find the repo's stdlib/. Point at it
        # explicitly via TYRA_STDLIB so `import io` etc. resolve.
        return Runner.run_cmd(
            [str(self._binary()), "build", str(src)],
            cwd=workdir,
            timeout_s=self.config["timeouts"]["compile_seconds"],
            env={"TYRA_STDLIB": str(self._stdlib_dir())},
        )

    def execute(self, workdir: Path, stdin: str, timeout_s: int) -> StageResult:
        # `tyra build main.ty` emits `main` (source stem, no extension).
        bin_path = workdir / "main"
        if not bin_path.exists():
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
