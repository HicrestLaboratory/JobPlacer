"""
cli_wrapper — Python library wrapper for the cli_wrapper Rust CLI.

Typical usage
-------------
    from cli_wrapper import JobPlacer, JobRequest, TopologySource, PlacementResult

    # Named system + topology file
    placer = JobPlacer(system="leonardo", topology_file="/path/to/topo.xml")

    # TOML file (system-agnostic, detected by extension)
    placer = JobPlacer(system="alps", topology=TopologySource.toml_file("/path/to/topo.toml"))

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
    """Represents a single job's placement requirements.

    The fields are serialised as-is into the JSON query consumed by the
    job_placer binary, so they must match whatever schema that binary expects.
    Add or remove fields here if the Rust-side schema changes.
    """
    num_nodes: int
    placement_class: Optional[str] = None
    extra: Dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        d = {
            "nodes": self.num_nodes,
        }
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
            # Expected shape: {"status": "Ok", "placements": {"job": {"nodes": [...]}}}
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
        """Load topology from a TOML file (system-agnostic, detected by .toml extension).

        The Rust CLI auto-detects TOML files by their extension and routes them
        through the TOML parser regardless of the --system flag.  You still need
        to pass ``system`` to :class:`JobPlacer` because the CLI requires it.
        """
        return _TomlFile(Path(path))

    @staticmethod
    def system_file(system: str, path: Union[str, Path]) -> "_SystemFile":
        """Load topology for a named system from a topology file.

        Parameters
        ----------
        system:
            Supported values: ``"leonardo"``, ``"jupiter"``, ``"alps"``.
        path:
            Path to the system-specific topology file.
        """
        return _SystemFile(system=system, path=Path(path))

    @staticmethod
    def system_scontrol(system: str) -> "_SystemScontrol":
        """Discover topology for a named system via ``scontrol``."""
        return _SystemScontrol(system=system)


@dataclass
class _TomlFile:
    """TOML topology file — passed as --topology-file; system is still required by the CLI."""
    path: Path

    def system(self) -> Optional[str]:
        return None  # caller must supply system separately

    def _apply(self, cmd: List[str], system: str) -> None:
        cmd += ["--system", system, "--topology-file", str(self.path)]


@dataclass
class _SystemFile:
    system: str
    path: Path

    def _apply(self, cmd: List[str], system: str) -> None:  # system arg ignored; self.system wins
        cmd += ["--system", self.system, "--topology-file", str(self.path)]


@dataclass
class _SystemScontrol:
    system: str

    def _apply(self, cmd: List[str], system: str) -> None:
        cmd += ["--system", self.system, "--topology-scontrol"]


_AnyTopologySource = Union[_TomlFile, _SystemFile, _SystemScontrol]


# ---------------------------------------------------------------------------
# Main library class
# ---------------------------------------------------------------------------

class JobPlacer:
    """High-level Python interface to the job_placer binary.

    Parameters
    ----------
    system:
        The cluster system name.  Required.
        Currently supported: ``"leonardo"``, ``"jupiter"``, ``"alps"``.
    topology:
        A topology source object created via :class:`TopologySource` factory
        methods.  You may also use the shorthand keyword arguments below.
    topology_file:
        Shorthand: path to a system-specific topology file (or a ``.toml``
        file for system-agnostic TOML input).  Requires ``system``.
    topology_scontrol:
        Shorthand: discover topology via scontrol.  Requires ``system``.
    nodelist:
        Restrict placement to these hostnames (comma-separated string or list).
        When omitted the wrapper uses the SLURM environment if available.
        Mutually exclusive with ``all_nodes``.
    all_nodes:
        Consider all available nodes (disables nodelist filtering).
        Mutually exclusive with ``nodelist``.
    partition:
        Keep only nodes belonging to this partition (e.g. ``"boost_usr_prod"``).
    include_unavailable:
        Include draining / drained / down nodes instead of filtering them out.
    sinfo:
        Run ``sinfo`` live to get partition and node-state information.
        Mutually exclusive with ``sinfo_file``.
    sinfo_file:
        Path to a pre-captured ``sinfo`` output file.
        Mutually exclusive with ``sinfo``.
    seed:
        RNG seed for the placer (different seeds → different placements).
    verbose:
        Forward the binary's ``--verbose`` flag (logs to stderr).
    visualize:
        Enable graphical visualisation (``--visualize`` flag).
    binary:
        Path to the compiled ``job_placer`` binary.
        Defaults to ``job_placer`` on ``$PATH``, then the directory that
        contains this module.
    """

    def __init__(
        self,
        system: str,
        topology: Optional[_AnyTopologySource] = None,
        *,
        # Shorthand topology args
        topology_file: Optional[Union[str, Path]] = None,
        topology_scontrol: bool = False,
        # Node filtering
        nodelist: Optional[Union[str, List[str]]] = None,
        all_nodes: bool = False,
        partition: Optional[str] = None,
        include_unavailable: bool = False,
        # sinfo — at most one may be set
        sinfo: bool = False,
        sinfo_file: Optional[Union[str, Path]] = None,
        # Misc
        seed: Optional[int] = None,
        verbose: bool = False,
        visualize: bool = False,
        binary: Optional[Union[str, Path]] = None,
    ):
        if not system:
            raise ValueError("system= is required (e.g. 'leonardo', 'jupiter', 'alps').")
        self._system = system

        self._topology = self._resolve_topology(
            system, topology, topology_file, topology_scontrol
        )

        # Node filtering — mirror the CLI's conflicts_with = "nodelist"
        if nodelist and all_nodes:
            raise ValueError("nodelist and all_nodes are mutually exclusive.")
        if isinstance(nodelist, list):
            nodelist = ",".join(nodelist)
        self._nodelist = nodelist
        self._all_nodes = all_nodes
        self._partition = partition
        self._include_unavailable = include_unavailable

        # sinfo — mirror the CLI's conflicts_with = "sinfo"
        if sinfo and sinfo_file:
            raise ValueError("sinfo and sinfo_file are mutually exclusive.")
        self._sinfo = sinfo
        self._sinfo_file = Path(sinfo_file) if sinfo_file else None

        # Misc
        self._seed = seed
        self._verbose = verbose
        self._visualize = visualize
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
        svg_out: Optional[Union[str, Path]] = None,
    ) -> PlacementResult:
        """Run the placer for the given job requests.

        Parameters
        ----------
        jobs:
            Mapping of job-name → :class:`JobRequest` (or a plain dict that
            will be passed through as-is to the JSON query).
        seed:
            Per-call seed override (takes precedence over the instance seed).
        timeout:
            Maximum time in seconds to wait for the binary.
        extra_args:
            Raw extra CLI arguments appended verbatim (escape hatch).
        svg_out:
            If given, pass ``--out-svg <path>`` to write an SVG visualisation.
            Implies ``--visualize``.

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

        # svg_out implies visualize; wire it up via extra_args
        _extra = list(extra_args or [])
        if svg_out:
            _extra += ["--out-svg", str(Path(svg_out).resolve())]

        cmd = self._build_command(seed_override=seed, extra_args=_extra or None)

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

            # if self._verbose and proc.stderr:
            #     print(proc.stderr, end="", file=sys.stderr)

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

        # Topology flags (--system + --topology-file / --topology-scontrol)
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

        # sinfo source (mutually exclusive flags)
        if self._sinfo:
            cmd += ["--sinfo"]
        elif self._sinfo_file:
            cmd += ["--sinfo-file", str(self._sinfo_file)]

        # Seed
        effective_seed = seed_override if seed_override is not None else self._seed
        if effective_seed is not None:
            cmd += ["--seed", str(effective_seed)]

        # if self._verbose:
        #     cmd += ["--verbose"]

        if self._visualize:
            cmd += ["--visualize"]

        if extra_args:
            cmd += extra_args

        # Query is always passed via stdin (no positional arg needed)
        return cmd

    @staticmethod
    def _resolve_topology(
        system: str,
        topology: Optional[_AnyTopologySource],
        topology_file: Optional[Union[str, Path]],
        topology_scontrol: bool,
    ) -> _AnyTopologySource:
        """Turn the mixed shorthand kwargs into a single topology source."""
        # Count how many sources were explicitly provided
        shorthand_count = sum([
            topology_file is not None,
            topology_scontrol,
        ])

        if topology is not None and shorthand_count > 0:
            raise ValueError(
                "Specify either topology=TopologySource.…(…) or the shorthand "
                "keyword arguments (topology_file / topology_scontrol), not both."
            )
        if topology_file is not None and topology_scontrol:
            raise ValueError("topology_file and topology_scontrol are mutually exclusive.")

        if topology is not None:
            return topology

        if topology_scontrol:
            return _SystemScontrol(system=system)
        if topology_file is not None:
            path = Path(topology_file)
            if path.suffix == ".toml":
                return _TomlFile(path)
            return _SystemFile(system=system, path=path)

        raise ValueError(
            f"system={system!r} requires a topology source: pass topology_file=<PATH>, "
            "topology_scontrol=True, or topology=TopologySource.<method>(…)."
        )

    @staticmethod
    def _resolve_binary(binary: Optional[Union[str, Path]]) -> Path:
        if binary is not None:
            p = Path(binary)
            if not p.exists():
                raise FileNotFoundError(f"job_placer binary not found at: {p}")
            return p

        # 1. $PATH
        found = shutil.which("job_placer_placement_classes")
        if found:
            return Path(found)

        # 2. Next to this module
        local = Path(__file__).parent / "target" / "release" / "job_placer_placement_classes"
        if local.exists():
            return local

        raise FileNotFoundError(
            "job_placer binary not found on $PATH or next to the library.\n"
            "Build it with `cargo build --release` and ensure it is on $PATH, "
            "or pass binary=<PATH> to JobPlacer(…)."
        )