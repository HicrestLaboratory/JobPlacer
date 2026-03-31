#!/usr/bin/env python3
"""
lumi_to_toml.py  –  Convert a LUMI SLURM nodelist to a Dragonfly-topology TOML.

Usage:
    python lumi_to_toml.py nodelist.txt [output.toml]

If output path is omitted, writes to stdout.
"""

import sys
import re
from collections import defaultdict

# ---------------------------------------------------------------------------
# Group-assignment formulas
# ---------------------------------------------------------------------------

def lumi_c_group(nodenum: int) -> int:
    """LUMI-C cabinet index (0-based)."""
    return (nodenum - 1000) // 256


def lumi_g_group(nodenum: int) -> int:
    """
    LUMI-G cabinet index (0-based within LUMI-G, so real cabinet = 8 + result).
    Groups 0-22 have 124 nodes each; group 23 (last) has 126 nodes.
    Formula: min(floor((nodenum - 5000) / 124), 23)
    """
    return min((nodenum - 5000) // 124, 23)


def classify(nodenum: int) -> str:
    """Return 'C' for LUMI-C nodes, 'G' for LUMI-G nodes, None otherwise."""
    if 1000 <= nodenum < 5000:
        return "C"
    if 5000 <= nodenum < 10000:
        return "G"
    return None


# ---------------------------------------------------------------------------
# Switch-assignment formulas
# ---------------------------------------------------------------------------
# LUMI-C:
#   256 nodes/group, 16 switches/group (s0-s15), 16 nodes/switch
#   switch_index = node_within_group // 16
#
# LUMI-G:
#   Each node has 4 GPUs, each GPU connects to a different switch.
#   Groups 0-22: 31 switches (s0-s30), 124 nodes/group
#   Group 23   : 32 switches (s0-s31), 126 nodes/group
#   Switch mapping (ADJUST HERE if hardware layout differs):
#     GPU rail r of node n_local → switch index: (n_local % switches_per_rail) + r * switches_per_rail
#     where switches_per_rail = num_switches // 4  (integer division)
#   This spreads the 4 GPU rails evenly across the 31/32 switches.

def lumi_c_switch(nodenum: int) -> list[str]:
    """Return list with one switch id string for a LUMI-C node."""
    group_idx = lumi_c_group(nodenum)
    cabinet = group_idx             # 0-based cabinet for LUMI-C
    n_local = (nodenum - 1000) % 256
    sw_idx  = n_local // 16
    group_id  = f"x{1000 + cabinet}"
    switch_id = f"{group_id}s{sw_idx}"
    return [switch_id]


def lumi_g_switch(nodenum: int) -> list[str]:
    """
    Return list of 4 switch id strings (one per GPU rail) for a LUMI-G node.
    LUMI-G cabinets start at x1200 (= LUMI cabinet 8 + group index).
    """
    group_idx    = lumi_g_group(nodenum)
    cabinet      = 8 + group_idx          # absolute cabinet number
    is_last      = (group_idx == 23)
    num_switches = 32 if is_last else 31
    switches_per_rail = num_switches // 4  # 8 for normal groups, 8 for last

    # Node offset within the group
    base = 5000 + group_idx * 124
    n_local = nodenum - base              # 0-based within group

    group_id = f"x{1200 + group_idx}"    # e.g. x1200, x1201, …

    switch_ids = []
    for rail in range(4):
        sw_idx = (n_local % switches_per_rail) + rail * switches_per_rail
        switch_ids.append(f"{group_id}s{sw_idx}")
    return switch_ids


# ---------------------------------------------------------------------------
# TOML serialisation helpers (no external deps)
# ---------------------------------------------------------------------------

def toml_str(v: str) -> str:
    return f'"{v}"'


def toml_str_list(lst: list[str]) -> str:
    return "[" + ", ".join(toml_str(x) for x in lst) + "]"


def write_section(lines: list[str], header: str, fields: dict):
    lines.append(header)
    for k, v in fields.items():
        lines.append(f"{k} = {v}")
    lines.append("")


# ---------------------------------------------------------------------------
# Main conversion
# ---------------------------------------------------------------------------

def parse_nodelist(path: str):
    """
    Parse the nodelist text file.
    Returns list of dicts: {nodenum, nodeid, features, partitions}
    Nodes appearing in multiple partitions are merged into one entry.
    """
    nodes: dict[int, dict] = {}

    with open(path) as f:
        for raw_line in f:
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split()
            if len(parts) < 3:
                continue
            nodeid, features, partition = parts[0], parts[1], parts[2]
            # Skip header
            if nodeid.upper() == "NODELIST":
                continue
            m = re.match(r"nid0*(\d+)", nodeid)
            if not m:
                continue
            nodenum = int(m.group(1))
            if nodenum not in nodes:
                nodes[nodenum] = {
                    "nodenum":    nodenum,
                    "nodeid":     nodeid,
                    "features":   features,
                    "partitions": [],
                }
            if partition not in nodes[nodenum]["partitions"]:
                nodes[nodenum]["partitions"].append(partition)

    return sorted(nodes.values(), key=lambda n: n["nodenum"])


def convert(nodelist_path: str) -> str:
    nodes = parse_nodelist(nodelist_path)

    # Collect all switch ids per group, preserving insertion order
    switches_by_group: dict[str, list[str]] = defaultdict(list)
    node_entries: list[dict] = []

    for node in nodes:
        nn   = node["nodenum"]
        kind = classify(nn)
        if kind == "C":
            sw_ids   = lumi_c_switch(nn)
            group_id = f"x{1000 + lumi_c_group(nn)}"
        elif kind == "G":
            sw_ids   = lumi_g_switch(nn)
            group_id = f"x{1200 + lumi_g_group(nn)}"
        else:
            # Unknown range – put in a catch-all group
            sw_ids   = [f"xUNKs0"]
            group_id = "xUNK"

        for sw in sw_ids:
            if sw not in switches_by_group[group_id]:
                switches_by_group[group_id].append(sw)

        node_entries.append({
            "nodeid":     node["nodeid"],
            "switch":     sw_ids,
            "group":      group_id,
            "partitions": node["partitions"],
        })

    # -------------------------------------------------------------------
    # Build output lines
    # -------------------------------------------------------------------
    lines: list[str] = []

    lines.append("# LUMI Dragonfly topology -- generated by lumi_to_toml.py")
    lines.append("# Hand-edit freely.")
    lines.append("")

    lines.append("[meta]")
    lines.append('system = "lumi"')
    lines.append("w_node_switch   = 1.0")
    lines.append("w_switch_switch = 1.0")
    lines.append("")

    lines.append("# -- Switches -------------------------------------------------------")
    lines.append("# Each switch belongs to one Dragonfly group (= cabinet).")
    lines.append("")

    for group_id in sorted(switches_by_group):
        for sw_id in switches_by_group[group_id]:
            write_section(lines, "[[switch]]", {
                "id":    toml_str(sw_id),
                "group": toml_str(group_id),
            })

    lines.append("# -- Nodes ---------------------------------------------------------")
    lines.append("")

    # Group nodes by (partition-tuple, group) for readability
    # but emit flat [[node]] entries as in the example
    current_group = None
    current_parts = None

    # Sort by group then by partitions then by nodenum
    node_entries.sort(key=lambda e: (e["group"], tuple(sorted(e["partitions"])), e["nodeid"]))

    for entry in node_entries:
        group_key = entry["group"]
        parts_key = tuple(sorted(entry["partitions"]))

        if group_key != current_group or parts_key != current_parts:
            label = f"# group={group_key}  partitions={list(parts_key)}"
            lines.append(label)
            current_group = group_key
            current_parts = parts_key

        fields = {
            "id":         toml_str(entry["nodeid"]),
            "switch":     toml_str_list(entry["switch"]),
            "group":      toml_str(entry["group"]),
            "partitions": toml_str_list(entry["partitions"]),
        }
        write_section(lines, "[[node]]", fields)

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} nodelist.txt [output.toml]", file=sys.stderr)
        sys.exit(1)

    toml_text = convert(sys.argv[1])

    if len(sys.argv) >= 3:
        with open(sys.argv[2], "w") as f:
            f.write(toml_text)
        print(f"Written to {sys.argv[2]}", file=sys.stderr)
    else:
        print(toml_text)


if __name__ == "__main__":
    main()