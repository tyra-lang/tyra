from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Optional


@dataclass
class StageResult:
    ok: bool
    duration_ms: int
    stdout: str = ""
    stderr: str = ""
    exit_code: Optional[int] = None
    note: str = ""


@dataclass
class RunResult:
    compile: Optional[StageResult] = None
    execute: Optional[StageResult] = None
    checks_passed: Optional[bool] = None
    check_details: List[str] = field(default_factory=list)


class Runner:
    name: str = "base"
    source_ext: str = ".txt"

    def __init__(self, config: dict, repo_root: Path):
        self.config = config
        self.repo_root = repo_root

    def compiler_available(self) -> bool:
        raise NotImplementedError

    def prepare_workdir(self, code: str) -> Path:
        """Write source into a throwaway workdir and return the dir."""
        workdir = Path(tempfile.mkdtemp(prefix=f"aigen-{self.name}-"))
        src = workdir / f"main{self.source_ext}"
        src.write_text(code)
        return workdir

    def cleanup(self, workdir: Path) -> None:
        shutil.rmtree(workdir, ignore_errors=True)

    def compile(self, workdir: Path) -> StageResult:
        raise NotImplementedError

    def execute(self, workdir: Path, stdin: str, timeout_s: int) -> StageResult:
        raise NotImplementedError

    @staticmethod
    def run_cmd(
        cmd: List[str],
        cwd: Path,
        stdin: Optional[str] = None,
        timeout_s: int = 30,
        env: Optional[dict] = None,
    ) -> StageResult:
        t0 = time.time()
        try:
            proc = subprocess.run(
                cmd,
                cwd=str(cwd),
                input=stdin,
                capture_output=True,
                text=True,
                timeout=timeout_s,
                env={**os.environ, **(env or {})},
            )
            dur = int((time.time() - t0) * 1000)
            return StageResult(
                ok=(proc.returncode == 0),
                duration_ms=dur,
                stdout=proc.stdout,
                stderr=proc.stderr,
                exit_code=proc.returncode,
            )
        except subprocess.TimeoutExpired as e:
            dur = int((time.time() - t0) * 1000)
            return StageResult(
                ok=False,
                duration_ms=dur,
                stdout=(e.stdout or b"").decode("utf-8", "replace")
                if isinstance(e.stdout, (bytes, bytearray))
                else (e.stdout or ""),
                stderr=f"timeout after {timeout_s}s",
                exit_code=None,
                note="timeout",
            )
        except FileNotFoundError as e:
            dur = int((time.time() - t0) * 1000)
            return StageResult(
                ok=False,
                duration_ms=dur,
                stderr=f"command not found: {cmd[0]} ({e})",
                note="compiler_missing",
            )
        except Exception as e:
            dur = int((time.time() - t0) * 1000)
            return StageResult(
                ok=False,
                duration_ms=dur,
                stderr=f"{type(e).__name__}: {e}",
            )

    @staticmethod
    def evaluate_checks(
        stdout: str,
        must_contain: List[str],
        must_not_contain: List[str],
    ) -> tuple[bool, List[str]]:
        details: List[str] = []
        ok = True
        for marker in must_contain:
            if marker not in stdout:
                ok = False
                details.append(f"missing marker: {marker!r}")
        for marker in must_not_contain:
            if marker in stdout:
                ok = False
                details.append(f"forbidden marker present: {marker!r}")
        return ok, details
