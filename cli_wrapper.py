"""
job_placer — Python library wrapper for the job_placer Rust CLI.

Typical usage
-------------
    from job_placer import JobPlacer, JobRequest, TopologySource, PlacementResult

    placer = JobPlacer(system="leonardo", topology_file="/path/to/topo.xml")

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
    job_kind: str
    placement_class: Optional[str] = None 
    extra: Dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        d = {
            "nodes": self.num_nodes,
            "job_kind": self.job_kind,
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
    def yaml_file(path: Union[str, Path]) -> "_YamlFile":
        """Load topology from a plain YAML file (system-agnostic)."""
        return _YamlFile(Path(path))

    @staticmethod
    def system_file(system: str, path: Union[str, Path]) -> "_SystemFile":
        """Load topology for a named system from a topology file.

        Parameters
        ----------
        system:
            Supported values: ``"leonardo"``, ``"jupiter"``.
        path:
            Path to the system-specific topology file.
        """
        return _SystemFile(system=system, path=Path(path))

    @staticmethod
    def system_scontrol(system: str) -> "_SystemScontrol":
        """Discover topology for a named system via ``scontrol``."""
        return _SystemScontrol(system=system)


@dataclass
class _YamlFile:
    path: Path

    def _apply(self, cmd: List[str]) -> None:
        cmd += ["--topology-yaml", str(self.path)]


@dataclass
class _SystemFile:
    system: str
    path: Path

    def _apply(self, cmd: List[str]) -> None:
        cmd += ["--system", self.system, "--topology-file", str(self.path)]


@dataclass
class _SystemScontrol:
    system: str

    def _apply(self, cmd: List[str]) -> None:
        cmd += ["--system", self.system, "--topology-scontrol"]


_AnyTopologySource = Union[_YamlFile, _SystemFile, _SystemScontrol]


# ---------------------------------------------------------------------------
# Main library class
# ---------------------------------------------------------------------------

class JobPlacer:
    """High-level Python interface to the job_placer binary.

    Parameters
    ----------
    topology:
        A topology source object created via :class:`TopologySource` factory
        methods.  You may also use the shorthand keyword arguments below.
    system:
        Shorthand: named system (``"leonardo"`` or ``"jupiter"``).
        Must be combined with exactly one of ``topology_file`` or
        ``topology_scontrol=True``.
    topology_yaml:
        Shorthand: path to a YAML topology file (system-agnostic).
    topology_file:
        Shorthand: path to a system-specific topology file.
        Requires ``system``.
    topology_scontrol:
        Shorthand: discover topology via scontrol.  Requires ``system``.
    nodelist:
        Restrict placement to these hostnames (comma-separated string or list).
        When omitted the wrapper uses the SLURM environment if available.
    all_nodes:
        Consider all available nodes (disables nodelist filtering).
    partition:
        Keep only nodes belonging to this partition.
    include_unavailable:
        Include draining / drained / down nodes.
    sinfo:
        Run ``sinfo`` live for partition / state enrichment.
    sinfo_file:
        Path to a pre-captured ``sinfo`` output file.
    seed:
        RNG seed for the placer (different seeds → different placements).
    verbose:
        Forward the binary's ``--verbose`` flag (logs to stderr).
    binary:
        Path to the compiled ``job_placer`` binary.
        Defaults to ``job_placer`` on ``$PATH``, then the directory that
        contains this module.
    """

    def __init__(
        self,
        topology: Optional[_AnyTopologySource] = None,
        *,
        # Shorthand topology args
        system: Optional[str] = None,
        topology_yaml: Optional[Union[str, Path]] = None,
        topology_file: Optional[Union[str, Path]] = None,
        topology_scontrol: bool = False,
        # Node filtering
        nodelist: Optional[Union[str, List[str]]] = None,
        all_nodes: bool = False,
        partition: Optional[str] = None,
        include_unavailable: bool = False,
        # sinfo
        sinfo: bool = False,
        sinfo_file: Optional[Union[str, Path]] = None,
        # Misc
        seed: Optional[int] = None,
        verbose: bool = False,
        binary: Optional[Union[str, Path]] = None,
    ):
        self._topology = self._resolve_topology(
            topology, system, topology_yaml, topology_file, topology_scontrol
        )

        # Node filtering
        if isinstance(nodelist, list):
            nodelist = ",".join(nodelist)
        self._nodelist = nodelist
        self._all_nodes = all_nodes
        self._partition = partition
        self._include_unavailable = include_unavailable

        # sinfo
        if sinfo and sinfo_file:
            raise ValueError("sinfo and sinfo_file are mutually exclusive.")
        self._sinfo = sinfo
        self._sinfo_file = Path(sinfo_file) if sinfo_file else None

        # Misc
        self._seed = seed
        self._verbose = verbose
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
        svg_out: Optional[Path] = None
    ) -> PlacementResult:
        """Run the placer for the given job requests.

        Parameters
        ----------
        jobs:
            Mapping of job-name → :class:`JobRequest` (or a plain dict that
            will be passed through as-is to the JSON query).
        seed:
            Per-call seed override (takes precedence over the instance seed).
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
        
        if svg_out:
            if not extra_args:
                extra_args = []
            extra_args += ['--out-svg', str(svg_out.resolve())]

        cmd = self._build_command(seed_override=seed, extra_args=extra_args)
        # print(' '.join(cmd))
        
        try:
            proc = subprocess.run(
                cmd,
                input=query_json,
                capture_output=True,
                text=True,
                timeout=timeout,
            )

            if self._verbose and proc.stderr:
                print(proc.stderr, end="", file=sys.stderr)

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
            return PlacementResult(ok=False, reason=f'timeout {timeout}s')
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

        # Topology
        self._topology._apply(cmd)

        # Node filtering
        if self._all_nodes:
            cmd += ["--all-nodes"]
        elif self._nodelist:
            cmd += ["--nodelist", self._nodelist]

        if self._partition:
            cmd += ["--partition", self._partition]
        if self._include_unavailable:
            cmd += ["--include-unavailable"]

        # sinfo
        if self._sinfo:
            cmd += ["--sinfo"]
        elif self._sinfo_file:
            cmd += ["--sinfo-file", str(self._sinfo_file)]

        # Seed
        effective_seed = seed_override if seed_override is not None else self._seed
        if effective_seed is not None:
            cmd += ["--seed", str(effective_seed)]

        if self._verbose:
            cmd += ["--verbose"]

        if extra_args:
            cmd += extra_args

        # Query is always passed via stdin (no positional arg)
        return cmd

    @staticmethod
    def _resolve_topology(
        topology: Optional[_AnyTopologySource],
        system: Optional[str],
        topology_yaml: Optional[Union[str, Path]],
        topology_file: Optional[Union[str, Path]],
        topology_scontrol: bool,
    ) -> _AnyTopologySource:
        """Turn the mixed shorthand kwargs into a single topology source."""
        sources_given = sum([
            topology is not None,
            topology_yaml is not None,
            system is not None,
        ])
        if sources_given == 0:
            raise ValueError(
                "A topology source is required. Use topology=TopologySource.yaml_file(…), "
                "or pass system=… / topology_yaml=… shorthand arguments."
            )
        if sources_given > 1:
            raise ValueError(
                "Only one topology source may be specified at a time."
            )

        if topology is not None:
            return topology

        if topology_yaml is not None:
            return _YamlFile(Path(topology_yaml))

        # system= branch
        if system is not None:
            if topology_scontrol and topology_file:
                raise ValueError("topology_file and topology_scontrol are mutually exclusive.")
            if topology_scontrol:
                return _SystemScontrol(system=system)
            if topology_file:
                return _SystemFile(system=system, path=Path(topology_file))
            raise ValueError(
                f"system={system!r} requires either topology_file=<PATH> or topology_scontrol=True."
            )

        raise ValueError("Could not resolve topology source.")  # unreachable

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
        local = Path(__file__).parent / "job_placer_placement_classes"
        if local.exists():
            return local

        raise FileNotFoundError(
            "job_placer binary not found on $PATH or next to the library.\n"
            "Build it with `cargo build --release` and ensure it is on $PATH, "
            "or pass binary=<PATH> to JobPlacer(…)."
        )