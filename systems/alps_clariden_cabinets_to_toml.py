#!/usr/bin/env python3
"""
alps_compute_edge_to_toml.py

Convert a "wide" Alps/Clariden topology CSV -- one row per device, listing
all of its neighbours as repeating (xname, distance) pairs -- into the
common Dragonfly TOML format used by the Rust topology tooling.

Only compute <-> edge-switch links are considered:
  - rows where kind == "comp"
  - neighbour pairs whose distance field is "-" (the edge / NIC<->switch
    links; local and global switch-switch links and any "other" /
    external links are ignored entirely)

Each compute node is assumed to have its NICs split across exactly two
edge switches ("a pair"). Across all compute nodes this pairing must be
globally consistent: if switch A is paired with switch B for one node,
then *every* other node that touches A must also be paired with B (and
vice versa) -- i.e. the edge switches partition cleanly into disjoint
pairs. If a switch is ever seen paired with two different partners, that's
treated as an inconsistency and an error is raised.

The Dragonfly/Slingshot group for each virtual switch is taken from the
`group` field (CSV column 3) of the compute-node rows themselves. All
nodes that share a switch pair must report the same group value; a
mismatch is treated as an inconsistency and an error is raised.

Each distinct pair becomes one "virtual" switch in the output, since the
downstream tooling models every node as attached to a single switch.
"""

import sys
import argparse
import csv
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


# ---------------------------------------------------------------------------
# Data structures
# ---------------------------------------------------------------------------

@dataclass
class VirtualSwitchRecord:
    id: str                    # e.g. "x1103c4r1+x1103c4r7"
    group: str                 # Dragonfly/Slingshot group, e.g. "17" (from CSV col 3)
    members: tuple[str, str]   # the two real switch xnames it represents


@dataclass
class NodeRecord:
    id: str       # nid, e.g. "nid005394"
    switch: str   # virtual switch id
    xname: str    # node's own xname, e.g. "x1103c4s1b0n0"
    group: str    # Dragonfly/Slingshot group, e.g. "17" (from CSV col 3)


@dataclass
class _RawNode:
    """Intermediate record: one compute node with its resolved switch pair."""
    nid: str
    xname: str
    group: str              # Dragonfly/Slingshot group from CSV col 3, e.g. "17"
    pair: tuple[str, str]   # sorted real switch ids, e.g. ("x1103c4r1", "x1103c4r7")


class TopologyError(Exception):
    """Raised for irrecoverable inconsistencies in the input topology."""


# ---------------------------------------------------------------------------
# xname parsing helpers
# ---------------------------------------------------------------------------

_SWITCH_RE      = re.compile(r'^(x\d+c\d+)r\d+$')
_SWITCH_PORT_RE = re.compile(r'^(x\d+c\d+r\d+)j\d+$')


def parse_switch_id(token: str) -> Optional[str]:
    """Return the switch xname if `token` looks like one, else None.

    Accepts both plain switch xnames (x1103c4r7) and switch+port forms
    (x1103c4r7j10), returning the switch xname in either case.
    """
    m = _SWITCH_PORT_RE.match(token)
    if m:
        return m.group(1)
    if _SWITCH_RE.match(token):
        return token
    return None


# ---------------------------------------------------------------------------
# Parser
# ---------------------------------------------------------------------------

def parse_topology_csv(path: Path) -> tuple[list[_RawNode], list[str]]:
    """
    Parse the wide topology CSV and return one _RawNode per compute node
    (with its resolved edge-switch pair and Dragonfly group), plus a list
    of non-fatal warnings.

    Raises TopologyError if a compute node does not have exactly two
    distinct edge switches.
    """
    raw_nodes: list[_RawNode] = []
    warnings: list[str] = []
    seen_nids: set[str] = set()

    with open(path, newline="") as fh:
        reader = csv.reader(fh)
        for lineno, row in enumerate(reader, 1):
            if not row or not row[0]:
                continue
            if row[0] == "nid":          # header row
                continue
            if len(row) < 4:
                warnings.append(f"line {lineno}: fewer than 4 fields -- skipped")
                continue

            nid, xname, group, kind = row[0], row[1], row[2], row[3]
            if kind != "comp":
                continue  # only compute-node rows carry node<->switch edges

            rest = row[4:]
            if len(rest) % 2 != 0:
                warnings.append(
                    f"line {lineno}: odd number of neighbour fields for "
                    f"{nid} -- trailing field ignored"
                )
                rest = rest[:-1]

            switches: set[str] = set()
            for i in range(0, len(rest), 2):
                neighbour, dist = rest[i], rest[i + 1]
                if dist != "-":
                    continue  # not an edge (node<->switch) link
                sw = parse_switch_id(neighbour)
                if sw is None:
                    warnings.append(
                        f"line {lineno}: edge neighbour '{neighbour}' for "
                        f"{nid} does not look like a switch xname -- ignored"
                    )
                    continue
                switches.add(sw)

            if len(switches) != 2:
                raise TopologyError(
                    f"line {lineno}: node {nid} ({xname}) has "
                    f"{len(switches)} distinct edge switch(es) "
                    f"{sorted(switches)}, expected exactly 2"
                )

            sw_a, sw_b = sorted(switches)

            if nid in seen_nids:
                warnings.append(f"line {lineno}: duplicate nid '{nid}' -- ignored")
                continue
            seen_nids.add(nid)

            raw_nodes.append(
                _RawNode(nid=nid, xname=xname, group=group, pair=(sw_a, sw_b))
            )

    return raw_nodes, warnings


# ---------------------------------------------------------------------------
# Switch-pair consistency check / virtual-switch construction
# ---------------------------------------------------------------------------

def virtual_switch_id(pair: tuple[str, str]) -> str:
    """Deterministic id for a virtual switch, e.g. 'x1103c4r1+x1103c4r7'."""
    return "+".join(pair)


def build_topology(
    raw_nodes: list[_RawNode],
) -> tuple[dict[str, VirtualSwitchRecord], dict[str, NodeRecord]]:
    """
    Verify that the node<->switch-pair relation is globally consistent --
    every edge switch must be paired with exactly one other switch across
    *all* compute nodes -- and build the virtual-switch / node tables for
    emission.

    Raises TopologyError if:
      - the same switch is observed paired with two different partner
        switches, e.g. node 0 pairs (A, B) but node 22 pairs (A, C): A
        cannot then be abstracted into a single virtual switch together
        with a consistent partner; or
      - two nodes sharing the same switch pair report different `group`
        values (CSV column 3), so no single group can be assigned to the
        resulting virtual switch.
    """
    # partner[sw] = the one switch `sw` has been paired with so far
    partner: dict[str, str] = {}
    # origin[sw] = the first node that established partner[sw] (for errors)
    origin: dict[str, _RawNode] = {}

    for n in raw_nodes:
        sw_a, sw_b = n.pair
        for sw, other in ((sw_a, sw_b), (sw_b, sw_a)):
            seen = partner.get(sw)
            if seen is None:
                partner[sw] = other
                origin[sw] = n
            elif seen != other:
                first = origin[sw]
                raise TopologyError(
                    f"switch {sw} is paired with {seen} via node "
                    f"{first.nid} ({first.xname}), but node {n.nid} "
                    f"({n.xname}) pairs it with {other} instead -- "
                    f"switch pairs are not consistent across nodes"
                )

    # Each switch pair must map to a single Dragonfly group (CSV col 3).
    pair_group: dict[tuple[str, str], str] = {}
    pair_group_origin: dict[tuple[str, str], _RawNode] = {}
    for n in raw_nodes:
        existing = pair_group.get(n.pair)
        if existing is None:
            pair_group[n.pair] = n.group
            pair_group_origin[n.pair] = n
        elif existing != n.group:
            first = pair_group_origin[n.pair]
            raise TopologyError(
                f"switch pair {n.pair} has group {existing!r} via node "
                f"{first.nid} ({first.xname}), but node {n.nid} "
                f"({n.xname}) reports group {n.group!r} for the same "
                f"pair -- Dragonfly group is not consistent across nodes "
                f"sharing this switch pair"
            )

    # A consistent `partner` mapping is its own inverse, so it partitions
    # the edge switches into disjoint pairs. Collect the unique ones.
    switches: dict[str, VirtualSwitchRecord] = {}
    seen_pairs: set[tuple[str, str]] = set()
    for sw, other in partner.items():
        pair = tuple(sorted((sw, other)))
        if pair in seen_pairs:
            continue
        seen_pairs.add(pair)
        vid = virtual_switch_id(pair)
        switches[vid] = VirtualSwitchRecord(
            id=vid, group=pair_group[pair], members=pair
        )

    nodes: dict[str, NodeRecord] = {}
    for n in raw_nodes:
        vid = virtual_switch_id(n.pair)
        nodes[n.nid] = NodeRecord(id=n.nid, switch=vid, xname=n.xname, group=n.group)

    return switches, nodes


# ---------------------------------------------------------------------------
# TOML emitter (hand-rolled; no tomli-w dependency required)
# ---------------------------------------------------------------------------

def _toml_str(s: str) -> str:
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"') + '"'


def emit_toml(
    switches: dict[str, VirtualSwitchRecord],
    nodes: dict[str, NodeRecord],
    *,
    w_node_switch: float = 1.0,
    w_switch_switch: float = 1.0,
    system_name: str = "alps",
    out=sys.stdout,
) -> None:

    def p(s: str = "") -> None:
        print(s, file=out)

    p("# Alps Dragonfly topology -- generated by alps_compute_edge_to_toml.py")
    p("# Hand-edit freely; the Rust parser merges explicit [[link]] entries")
    p("# with synthesised intra-/inter-group links.")
    p("#")
    p("# Each [[switch]] below is a VIRTUAL switch representing a pair of")
    p("# real edge switches that compute nodes consistently have their")
    p("# NICs split across (modelled here as a single switch).")
    p()
    p("[meta]")
    p(f'system = {_toml_str(system_name)}')
    p(f"w_node_switch   = {w_node_switch}")
    p(f"w_switch_switch = {w_switch_switch}")
    p()

    p("# -- Switches (virtual) ----------------------------------------------")
    p("# `group` is the Dragonfly/Slingshot group from the input CSV (col 3),")
    p("# e.g. \"17\" -- shared by both real switches in `members`.")
    p()
    for sw in sorted(switches.values(), key=lambda s: s.id):
        p("[[switch]]")
        p(f"id    = {_toml_str(sw.id)}")
        p(f"group = {_toml_str(sw.group)}")
        p(f"# members: {sw.members[0]}, {sw.members[1]}")
        p()

    p("# -- Compute nodes -----------------------------------------------------")
    p()
    for node in sorted(nodes.values(), key=lambda n: n.id):
        p("[[node]]")
        p(f"id     = {_toml_str(node.id)}")
        p(f"switch = {_toml_str(node.switch)}")
        p(f"xname  = {_toml_str(node.xname)}")
        p(f"group  = {_toml_str(node.group)}")
        p()


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main() -> None:
    ap = argparse.ArgumentParser(
        description=(
            "Convert a wide Alps/Clariden topology CSV into the common "
            "Dragonfly TOML format, considering only compute<->edge-switch "
            "links and collapsing each node's switch pair into one virtual "
            "switch."
        )
    )
    ap.add_argument("input", type=Path, help="topology CSV input file")
    ap.add_argument(
        "output", type=Path, nargs="?",
        help="output TOML file (default: stdout)",
    )
    ap.add_argument(
        "--system", default="alps",
        help="system name written into [meta] (default: alps)",
    )
    ap.add_argument(
        "--w-node-switch", type=float, default=1.0,
        help="link weight node-switch, normalised (default: 1.0 = 200 Gb/s)",
    )
    ap.add_argument(
        "--w-switch-switch", type=float, default=1.0,
        help="link weight switch-switch, normalised (default: 1.0 = 200 Gb/s)",
    )
    args = ap.parse_args()

    try:
        raw_nodes, warnings = parse_topology_csv(args.input)
        for w in warnings:
            print(f"WARNING: {w}", file=sys.stderr)
        switches, nodes = build_topology(raw_nodes)
    except TopologyError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        sys.exit(1)

    if args.output:
        with open(args.output, "w") as fh:
            emit_toml(
                switches, nodes,
                w_node_switch=args.w_node_switch,
                w_switch_switch=args.w_switch_switch,
                system_name=args.system,
                out=fh,
            )
        print(
            f"Written {args.output} "
            f"({len(switches)} virtual switches, {len(nodes)} nodes)",
            file=sys.stderr,
        )
    else:
        emit_toml(
            switches, nodes,
            w_node_switch=args.w_node_switch,
            w_switch_switch=args.w_switch_switch,
            system_name=args.system,
        )


if __name__ == "__main__":
    main()