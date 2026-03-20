#!/usr/bin/env python3
"""
alps_to_toml.py -- Convert an Alps Slingshot topology file to the common
TOML intermediate format consumed by the Rust topology parser.

Input format (one entry per line):
  <switch>:<port>:<nic_a>:<nic_b>:<nid>

  e.g.  x1301c7r5j105:none:x1301c7s5b1n0h3:x1301c7s5b1n0h2:nid006565

  Fields:
    switch  -- xname of the switch  (x<cab>c<chassis>r<slot>)
    port    -- port label            (jNNN, or "none")
    nic_a   -- first NIC xname
    nic_b   -- second NIC xname (or "none")
    nid     -- Slurm node name  (nidNNNNNN)

Output: TOML file following the common topology format.

Alps Dragonfly topology:
  - One switch level (every switch is both ToR and Dragonfly participant).
  - Dragonfly group = chassis (x<cab>c<chassis>).
  - Intra-group and inter-group links are synthesised by the Rust parser
    unless overridden via [[link]] entries.

Link weights (normalised to 200 Gb/s base = 1.0):
  node-switch   : 1.0
  switch-switch : 1.0
"""

import sys
import argparse
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


# ---------------------------------------------------------------------------
# Duplicate policy
# ---------------------------------------------------------------------------

class DuplicatePolicy:
    FIRST = "first"
    LAST  = "last"


# ---------------------------------------------------------------------------
# Data structures
# ---------------------------------------------------------------------------

@dataclass
class SwitchRecord:
    id: str     # e.g. "x1001c1r3"
    group: str  # e.g. "x1001c1"  (chassis = dragonfly group)


@dataclass
class NodeRecord:
    id: str      # e.g. "nid001305"
    switch: str  # switch xname
    xname: str   # primary NIC xname


@dataclass
class LinkRecord:
    a: str
    b: str
    weight: float
    comment: Optional[str] = None


# ---------------------------------------------------------------------------
# xname parsing helpers
# ---------------------------------------------------------------------------

_SWITCH_RE      = re.compile(r'^(x\d+c\d+)r\d+$')
_SWITCH_PORT_RE = re.compile(r'^(x\d+c\d+r\d+)j\d+$')


def parse_switch_id(token: str) -> Optional[str]:
    m = _SWITCH_PORT_RE.match(token)
    if m:
        return m.group(1)
    if _SWITCH_RE.match(token):
        return token
    return None


def group_from_switch(switch_id: str) -> Optional[str]:
    m = re.match(r'^(x\d+c\d+)r\d+$', switch_id)
    return m.group(1) if m else None


def primary_nic(nic_a: str, nic_b: str) -> str:
    if nic_a and nic_a != "none":
        return nic_a
    if nic_b and nic_b != "none":
        return nic_b
    return nic_a


# ---------------------------------------------------------------------------
# Parser
# ---------------------------------------------------------------------------

def parse_alps_file(
    path: Path,
    duplicate_policy: str = DuplicatePolicy.FIRST,
) -> tuple[dict[str, SwitchRecord], dict[str, NodeRecord], list[str]]:
    switches: dict[str, SwitchRecord] = {}
    nodes:    dict[str, NodeRecord]   = {}
    warnings: list[str]               = []

    with open(path) as fh:
        for lineno, raw in enumerate(fh, 1):
            line = raw.strip()
            if not line or line.startswith('#'):
                continue

            parts = line.split(':')
            if len(parts) < 5:
                warnings.append(
                    f"line {lineno}: expected 5 colon-separated fields, "
                    f"got {len(parts)} -- skipped"
                )
                continue

            switch_token, _port, nic_a, nic_b, nid = (
                parts[0], parts[1], parts[2], parts[3], parts[4]
            )

            # --- Switch ---
            switch_id = parse_switch_id(switch_token)
            if switch_id is None:
                warnings.append(
                    f"line {lineno}: cannot parse switch from '{switch_token}' -- skipped"
                )
                continue

            group = group_from_switch(switch_id)
            if group is None:
                warnings.append(
                    f"line {lineno}: cannot derive group from switch '{switch_id}' -- skipped"
                )
                continue

            if switch_id not in switches:
                switches[switch_id] = SwitchRecord(id=switch_id, group=group)

            # --- Node ---
            if not nid or nid == "none":
                warnings.append(f"line {lineno}: missing nid -- skipped")
                continue

            nic = primary_nic(nic_a, nic_b)

            if nid in nodes:
                existing = nodes[nid]
                if existing.switch != switch_id:
                    if duplicate_policy == DuplicatePolicy.LAST:
                        warnings.append(
                            f"line {lineno}: node '{nid}' remapped from switch "
                            f"'{existing.switch}' to '{switch_id}' (--on-duplicate=last)"
                        )
                        nodes[nid] = NodeRecord(id=nid, switch=switch_id, xname=nic)
                    else:
                        warnings.append(
                            f"line {lineno}: node '{nid}' already mapped to switch "
                            f"'{existing.switch}', ignoring duplicate entry for switch "
                            f"'{switch_id}' (--on-duplicate=first)"
                        )
                continue

            nodes[nid] = NodeRecord(id=nid, switch=switch_id, xname=nic)

    return switches, nodes, warnings


# ---------------------------------------------------------------------------
# TOML emitter (hand-rolled; no tomli-w dependency required)
# ---------------------------------------------------------------------------

def _toml_str(s: str) -> str:
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"') + '"'


def emit_toml(
    switches: dict[str, SwitchRecord],
    nodes: dict[str, NodeRecord],
    extra_links: list[LinkRecord],
    *,
    w_node_switch: float = 1.0,
    w_switch_switch: float = 1.0,
    system_name: str = "alps",
    out=sys.stdout,
) -> None:

    def p(s: str = "") -> None:
        print(s, file=out)

    p("# Alps Dragonfly topology -- generated by alps_to_toml.py")
    p("# Hand-edit freely; the Rust parser merges explicit [[link]] entries")
    p("# with synthesised intra-/inter-group links.")
    p()
    p("[meta]")
    p(f'system = {_toml_str(system_name)}')
    p(f"w_node_switch   = {w_node_switch}")
    p(f"w_switch_switch = {w_switch_switch}")
    p()

    p("# -- Switches -------------------------------------------------------")
    p("# Each switch belongs to one Dragonfly group (= chassis, e.g. x1001c1).")
    p()
    for sw in sorted(switches.values(), key=lambda s: s.id):
        p("[[switch]]")
        p(f"id    = {_toml_str(sw.id)}")
        p(f"group = {_toml_str(sw.group)}")
        p()

    p("# -- Compute nodes --------------------------------------------------")
    p()
    for node in sorted(nodes.values(), key=lambda n: n.id):
        p("[[node]]")
        p(f"id     = {_toml_str(node.id)}")
        p(f"switch = {_toml_str(node.switch)}")
        p(f"xname  = {_toml_str(node.xname)}")
        p()

    if extra_links:
        p("# -- Explicit inter-switch links (optional overrides) ----------------")
        p("# The parser synthesises intra- and inter-group links automatically.")
        p("# Add entries here only to override weights or add non-standard links.")
        p()
        for lnk in extra_links:
            p("[[link]]")
            p(f"a      = {_toml_str(lnk.a)}")
            p(f"b      = {_toml_str(lnk.b)}")
            p(f"weight = {lnk.weight}")
            if lnk.comment:
                p(f"# {lnk.comment}")
            p()


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main() -> None:
    ap = argparse.ArgumentParser(
        description="Convert an Alps Slingshot topology file to the common TOML format."
    )
    ap.add_argument(
        "input",  type=Path,
        help="Alps topology input file",
    )
    ap.add_argument(
        "output", type=Path, nargs="?",
        help="Output TOML file (default: stdout)",
    )
    ap.add_argument(
        "--system", default="alps",
        help="System name written into [meta] (default: alps)",
    )
    ap.add_argument(
        "--w-node-switch", type=float, default=1.0,
        help="Link weight node-switch, normalised (default: 1.0 = 200 Gb/s)",
    )
    ap.add_argument(
        "--w-switch-switch", type=float, default=1.0,
        help="Link weight switch-switch, normalised (default: 1.0 = 200 Gb/s)",
    )
    ap.add_argument(
        "--on-duplicate",
        choices=[DuplicatePolicy.FIRST, DuplicatePolicy.LAST],
        default=DuplicatePolicy.LAST,
        help=(
            "How to handle a node mapped to multiple switches. "
            "'first' (default): keep the first mapping seen. "
            "'last': overwrite with the last mapping seen. "
            "Either way a WARNING is emitted to stderr."
        ),
    )
    args = ap.parse_args()

    switches, nodes, warnings = parse_alps_file(
        args.input,
        duplicate_policy=args.on_duplicate,
    )

    for w in warnings:
        print(f"WARNING: {w}", file=sys.stderr)

    if args.output:
        with open(args.output, "w") as fh:
            emit_toml(
                switches, nodes, [],
                w_node_switch=args.w_node_switch,
                w_switch_switch=args.w_switch_switch,
                system_name=args.system,
                out=fh,
            )
        print(
            f"Written {args.output}  ({len(switches)} switches, {len(nodes)} nodes)",
            file=sys.stderr,
        )
    else:
        emit_toml(
            switches, nodes, [],
            w_node_switch=args.w_node_switch,
            w_switch_switch=args.w_switch_switch,
            system_name=args.system,
        )


if __name__ == "__main__":
    main()