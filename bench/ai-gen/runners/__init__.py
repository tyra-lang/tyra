from .base import Runner, StageResult, RunResult
from .tyra import TyraRunner
from .crystal import CrystalRunner
from .v import VRunner
from .gleam import GleamRunner
from .ruby import RubyRunner

ALL_RUNNERS = {
    "tyra": TyraRunner,
    "crystal": CrystalRunner,
    "v": VRunner,
    "gleam": GleamRunner,
    "ruby": RubyRunner,
}

__all__ = [
    "Runner",
    "StageResult",
    "RunResult",
    "TyraRunner",
    "CrystalRunner",
    "VRunner",
    "GleamRunner",
    "RubyRunner",
    "ALL_RUNNERS",
]
