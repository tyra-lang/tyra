from __future__ import annotations

import shutil
from pathlib import Path

from .base import Runner, StageResult


class GleamRunner(Runner):
    """Gleam demands a project, not a lone file.

    Strategy: copy `templates/gleam/` into the workdir, drop the
    generated code into `src/aigen_bench.gleam`, then run
    `gleam build` and `gleam run`.
    """

    name = "gleam"
    source_ext = ".gleam"

    def compiler_available(self) -> bool:
        return shutil.which("gleam") is not None

    def prepare_workdir(self, code: str) -> Path:
        workdir = super().prepare_workdir(code)
        template = (
            Path(__file__).resolve().parent.parent / "templates" / "gleam"
        )
        # Copy template files (gleam.toml, etc.) into the workdir.
        for entry in template.iterdir():
            if entry.name == ".gitkeep":
                continue
            dest = workdir / entry.name
            if entry.is_dir():
                shutil.copytree(entry, dest, dirs_exist_ok=True)
            else:
                shutil.copy2(entry, dest)
        src_dir = workdir / "src"
        src_dir.mkdir(exist_ok=True)
        # The main module name must match the `name` in gleam.toml.
        (src_dir / "aigen_bench.gleam").write_text(code)
        # Remove the scratch main.gleam from the base prepare step so it
        # doesn't confuse the build.
        scratch = workdir / "main.gleam"
        if scratch.exists():
            scratch.unlink()
        return workdir

    def compile(self, workdir: Path) -> StageResult:
        return Runner.run_cmd(
            ["gleam", "build"],
            cwd=workdir,
            timeout_s=self.config["timeouts"]["compile_seconds"],
        )

    def execute(self, workdir: Path, stdin: str, timeout_s: int) -> StageResult:
        return Runner.run_cmd(
            ["gleam", "run"],
            cwd=workdir,
            stdin=stdin,
            timeout_s=timeout_s,
        )
