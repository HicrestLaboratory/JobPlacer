"""
cli_wrapper — Python library wrapper for the job_placer_placement_classes Rust CLI.

Typical usage
-------------
    from cli_wrapper import JobPlacer, JobRequest, TopologySource, PlacementResult

    # Named system + scontrol (default, no file needed)
    placer = JobPlacer(system="leonardo")

    # Named system + topology file
    placer = JobPlacer(system="leonardo", topology_file="/path/to/topo.xml")

    # TOML file (system-agnostic)
    placer = JobPlacer(system="alps", topology_toml_file="/path/to/topo.toml")

    result = placer.place({
        "train_a": JobRequest(num_nodes=4),
        "train_b": JobRequest(num_nodes=8, placement_class="intra-l1"),
    })

    if result.ok:
        for job, nodes in result.placements.items():
            print(job, nodes)
    else:
        print("Infeasible:", result.reason)
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Union


# ---------------------------------------------------------------------------
# Public data model
# ---------------------------------------------------------------------------

@dataclass
class JobRequest:
    """Represents a single job's placement requirements."""
    num_nodes: int
    placement_class: Optional[str] = None
    extra: Dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        d = {"nodes": self.num_nodes}
        if self.placement_class is not None:
            d["placement_class"] = self.placement_class
        d.update(self.extra)
        return d


@dataclass
class PlacementResult:
    """Parsed result returned by the job_placer binary.

    Attributes
    ----------
    ok:
        True when the binary exited 0 and returned a feasible placement.
    placements:
        Mapping of job-name → list[hostname].  Empty when ``ok`` is False.
    reason:
        Human-readable infeasibility message when ``ok`` is False.
    raw:
        The raw JSON dict returned by the binary (always present).
    """
    ok: bool
    reason: Optional[str]
    placements: Optional[Dict[str, List[str]]] = None
    raw: Optional[dict] = None

    @classmethod
    def _from_raw(cls, raw: dict, exit_code: int) -> "PlacementResult":
        ok = exit_code == 0 and raw.get("status") != "Infeasible"
        placements: Dict[str, List[str]] = {}
        reason: Optional[str] = None

        if ok:
            for job_name, placement in raw.get("placements", {}).items():
                placements[job_name] = placement.get("nodes", [])
        else:
            reason = raw.get("reason") or raw.get("message") or "Infeasible"

        return cls(ok=ok, placements=placements, reason=reason, raw=raw)


# ---------------------------------------------------------------------------
# Topology source helpers
# ---------------------------------------------------------------------------

class TopologySource:
    """Namespace for topology source factory methods — mirrors the CLI flags."""

    @staticmethod
    def toml_file(path: Union[str, Path]) -> "_TomlFile":
        """Load topology from a TOML file via ``--topology-toml-file``."""
        return _TomlFile(Path(path))

    @staticmethod
    def system_file(path: Union[str, Path]) -> "_SystemFile":
        """Load topology from a system-specific file via ``--topology-file``."""
        return _SystemFile(path=Path(path))

    @staticmethod
    def scontrol() -> "_SystemScontrol":
        """Discover topology via scontrol (default when no file is given)."""
        return _SystemScontrol()


@dataclass
class _TomlFile:
    """TOML topology file — passed as ``--topology-toml-file <path>``."""
    path: Path

    def _apply(self, cmd: List[str], system: str) -> None:
        cmd += ["--system", system, "--topology-toml-file", str(self.path)]


@dataclass
class _SystemFile:
    """System-specific topology file — passed as ``--topology-file <path>``."""
    path: Path

    def _apply(self, cmd: List[str], system: str) -> None:
        cmd += ["--system", system, "--topology-file", str(self.path)]


@dataclass
class _SystemScontrol:
    """Default topology source: scontrol (no extra flags needed)."""

    def _apply(self, cmd: List[str], system: str) -> None:
        # scontrol is the default when neither --topology-file nor
        # --topology-toml-file is passed; just set the system.
        cmd += ["--system", system]


_AnyTopologySource = Union[_TomlFile, _SystemFile, _SystemScontrol]


# ---------------------------------------------------------------------------
# Main library class
# ---------------------------------------------------------------------------

class JobPlacer:
    """High-level Python interface to the job_placer_placement_classes binary.

    Parameters
    ----------
    system:
        The cluster system name (``"leonardo"``, ``"jupiter"``, ``"alps"``).
    topology:
        A topology source object created via :class:`TopologySource` factory
        methods.  You may also use the shorthand keyword arguments below.
    topology_file:
        Shorthand: path to a system-specific topology file
        (``--topology-file``).  Mutually exclusive with ``topology_toml_file``.
    topology_toml_file:
        Shorthand: path to a TOML topology file (``--topology-toml-file``).
        Mutually exclusive with ``topology_file``.
    nodelist:
        Restrict placement to these hostnames (comma-separated string or list).
        Mutually exclusive with ``all_nodes``.
    all_nodes:
        Consider all available nodes.  Mutually exclusive with ``nodelist``.
    partition:
        Keep only nodes belonging to this partition.
    include_unavailable:
        Include draining / drained / down nodes.
    sinfo_file:
        Path to a pre-captured ``sinfo`` output file (``--sinfo-file``).
        When omitted, sinfo runs live automatically.
    seed:
        RNG seed for the placer.
    verbose:
        Forward the binary's ``--verbose`` flag.
    visualize:
        Enable graphical visualisation (``--visualize`` flag).
    out_svg:
        Write an SVG visualisation to this path (``--out-svg``).
        Implies ``visualize=True``.
    binary:
        Path to the compiled binary.  Defaults to ``job_placer_placement_classes``
        on ``$PATH``, then ``<module_dir>/target/release/``.
    """

    def __init__(
        self,
        system: str,
        topology: Optional[_AnyTopologySource] = None,
        *,
        # Shorthand topology args
        topology_file: Optional[Union[str, Path]] = None,
        topology_toml_file: Optional[Union[str, Path]] = None,
        # Node filtering
        nodelist: Optional[Union[str, List[str]]] = None,
        all_nodes: bool = False,
        partition: Optional[str] = None,
        include_unavailable: bool = False,
        # sinfo
        sinfo_file: Optional[Union[str, Path]] = None,
        # Misc
        seed: Optional[int] = None,
        verbose: bool = False,
        visualize: bool = False,
        out_svg: Optional[Union[str, Path]] = None,
        binary: Optional[Union[str, Path]] = None,
    ):
        if not system:
            raise ValueError("system= is required (e.g. 'leonardo', 'jupiter', 'alps').")
        self._system = system

        self._topology = self._resolve_topology(
            system, topology, topology_file, topology_toml_file
        )

        if nodelist and all_nodes:
            raise ValueError("nodelist and all_nodes are mutually exclusive.")
        if isinstance(nodelist, list):
            nodelist = ",".join(nodelist)
        self._nodelist = nodelist
        self._all_nodes = all_nodes
        self._partition = partition
        self._include_unavailable = include_unavailable

        self._sinfo_file = Path(sinfo_file) if sinfo_file else None

        self._seed = seed
        self._verbose = verbose
        self._visualize = visualize or (out_svg is not None)
        self._out_svg = Path(out_svg).resolve() if out_svg else None
        self._binary = self._resolve_binary(binary)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def place(
        self,
        jobs: Dict[str, Union[JobRequest, dict]],
        *,
        seed: Optional[int] = None,
        timeout: float = 5.0,
        extra_args: Optional[List[str]] = None,
    ) -> PlacementResult:
        """Run the placer for the given job requests.

        Parameters
        ----------
        jobs:
            Mapping of job-name → :class:`JobRequest` (or a plain dict).
        seed:
            Per-call seed override (takes precedence over the instance seed).
        timeout:
            Maximum time in seconds to wait for the binary.
        extra_args:
            Raw extra CLI arguments appended verbatim (escape hatch).

        Returns
        -------
        PlacementResult
        """
        query = {
            name: (req.to_dict() if isinstance(req, JobRequest) else req)
            for name, req in jobs.items()
        }
        query_json = json.dumps(query)
        if self._verbose:
            print(query_json)

        cmd = self._build_command(seed_override=seed, extra_args=extra_args)

        if self._verbose:
            print(" ".join(cmd), file=sys.stderr)

        try:
            proc = subprocess.run(
                cmd,
                input=query_json,
                capture_output=True,
                text=True,
                timeout=timeout,
            )

            if not proc.stdout.strip():
                raise RuntimeError(
                    f"job_placer produced no output (exit {proc.returncode}).\n"
                    f"stderr: {proc.stderr.strip()}"
                )

            try:
                raw = json.loads(proc.stdout)
            except json.JSONDecodeError as exc:
                raise RuntimeError(
                    f"Failed to parse job_placer output as JSON: {exc}\n"
                    f"stdout: {proc.stdout[:500]}"
                ) from exc

            return PlacementResult._from_raw(raw, proc.returncode)

        except subprocess.TimeoutExpired:
            return PlacementResult(ok=False, reason=f"timeout after {timeout}s")
        except Exception as exc:
            return PlacementResult(ok=False, reason=f"Error: {exc}")

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _build_command(
        self,
        seed_override: Optional[int],
        extra_args: Optional[List[str]],
    ) -> List[str]:
        cmd: List[str] = [str(self._binary)]

        # Topology flags (--system + optional file flag)
        self._topology._apply(cmd, self._system)

        # Node filtering
        if self._all_nodes:
            cmd += ["--all-nodes"]
        elif self._nodelist:
            cmd += ["--nodelist", self._nodelist]

        if self._partition:
            cmd += ["--partition", self._partition]
        if self._include_unavailable:
            cmd += ["--include-unavailable"]

        # sinfo file (omitting this flag means sinfo runs live)
        if self._sinfo_file:
            cmd += ["--sinfo-file", str(self._sinfo_file)]

        # Seed
        effective_seed = seed_override if seed_override is not None else self._seed
        if effective_seed is not None:
            cmd += ["--seed", str(effective_seed)]

        if self._verbose:
            cmd += ["--verbose"]

        if self._visualize:
            cmd += ["--visualize"]

        if self._out_svg:
            cmd += ["--out-svg", str(self._out_svg)]

        if extra_args:
            cmd += extra_args

        return cmd

    @staticmethod
    def _resolve_topology(
        system: str,
        topology: Optional[_AnyTopologySource],
        topology_file: Optional[Union[str, Path]],
        topology_toml_file: Optional[Union[str, Path]],
    ) -> _AnyTopologySource:
        """Turn the mixed shorthand kwargs into a single topology source."""
        shorthand_count = sum([
            topology_file is not None,
            topology_toml_file is not None,
        ])

        if topology is not None and shorthand_count > 0:
            raise ValueError(
                "Specify either topology=TopologySource.…(…) or the shorthand "
                "keyword arguments (topology_file / topology_toml_file), not both."
            )
        if topology_file is not None and topology_toml_file is not None:
            raise ValueError("topology_file and topology_toml_file are mutually exclusive.")

        if topology is not None:
            return topology

        if topology_toml_file is not None:
            return _TomlFile(Path(topology_toml_file))
        if topology_file is not None:
            return _SystemFile(path=Path(topology_file))

        # Default: use scontrol (no file flags passed to the CLI)
        return _SystemScontrol()

    @staticmethod
    def _resolve_binary(binary: Optional[Union[str, Path]]) -> Path:
        if binary is not None:
            p = Path(binary)
            if not p.exists():
                raise FileNotFoundError(f"job_placer binary not found at: {p}")
            return p

        found = shutil.which("job_placer_placement_classes")
        if found:
            return Path(found)

        local = Path(__file__).parent / "target" / "release" / "job_placer_placement_classes"
        if local.exists():
            return local

        raise FileNotFoundError(
            "job_placer binary not found on $PATH or next to the library.\n"
            "Build it with `cargo build --release` and ensure it is on $PATH, "
            "or pass binary=<PATH> to JobPlacer(…)."
        )