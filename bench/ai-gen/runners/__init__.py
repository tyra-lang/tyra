from .base import Runner, StageResult, RunResult
from .tyra import TyraRunner
from .crystal import CrystalRunner
from .v import VRunner
from .gleam import GleamRunner
from .ruby import RubyRunner
from .go import GoRunner

ALL_RUNNERS = {
    "tyra": TyraRunner,
    "crystal": CrystalRunner,
    "v": VRunner,
    "gleam": GleamRunner,
    "ruby": RubyRunner,
    "go": GoRunner,
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
    "GoRunner",
    "ALL_RUNNERS",
]
