#!/usr/bin/env python3
"""
lumi_to_toml.py  –  Convert a LUMI SLURM nodelist to a Dragonfly-topology TOML.

Usage:
    python lumi_to_toml.py nodelist.txt [output.toml]

If output path is omitted, writes to stdout.

Input format (modern SLURM):
    NodeName=nid00[5000-5123] Feature=AMD_EPYC_7A53,x1100,eessi Gres=gpu:mi250:8
    Sockets=8 CoresPerSocket=8 CPUSpecList=0,16,32,48,64,80,96,112
    NodeName=nid00[5124-5247] Feature=AMD_EPYC_7A53,x1101,eessi Gres=gpu:mi250:8
    ...
"""

import sys
import re
from collections import defaultdict

# ---------------------------------------------------------------------------
# Cabinet and group assignment
# ---------------------------------------------------------------------------

def extract_cabinet_from_features(features: str) -> str:
    """
    Extract cabinet ID from Features string.
    Looks for pattern like x1100, x1101, etc.
    Returns cabinet ID (e.g. 'x1100') or None if not found.
    """
    m = re.search(r'(x[0-9]{4})', features)
    if m:
        return m.group(1)
    return None


def expand_nodename_range(nodename: str) -> list[str]:
    """
    Expand node name ranges like nid00[5000-5123] to individual node names.
    Returns list of node names like ['nid005000', 'nid005001', ..., 'nid005123']
    """
    m = re.match(r'(nid0*)(\[(\d+)-(\d+)\])', nodename)
    if not m:
        # Not a range, return as-is
        return [nodename]
    
    prefix = m.group(1)  # e.g. "nid00"
    start = int(m.group(3))
    end = int(m.group(4))
    
    return [f"{prefix}{i}" for i in range(start, end + 1)]


def parse_nodelist(path: str) -> list[dict]:
    """
    Parse modern SLURM nodelist format.
    Returns list of dicts: {nodename, nodenum, cabinet, features}
    
    Format:
        NodeName=nid00[5000-5123] Feature=AMD_EPYC_7A53,x1100,eessi Gres=gpu:mi250:8
        Sockets=8 CoresPerSocket=8 CPUSpecList=0,16,32,48,64,80,96,112
        NodeName=nid00[5124-5247] Feature=AMD_EPYC_7A53,x1101,eessi ...
    """
    nodes = []
    
    with open(path) as f:
        lines = f.readlines()
    
    i = 0
    while i < len(lines):
        line = lines[i].strip()
        i += 1
        
        if not line or line.startswith("#"):
            continue
        
        # Look for NodeName= line
        if not line.startswith("NodeName="):
            continue
        
        # Parse NodeName and Feature from this line
        nodename_match = re.search(r'NodeName=(\S+)', line)
        feature_match = re.search(r'Feature=(\S+)', line)
        
        if not nodename_match or not feature_match:
            continue
        
        nodename_pattern = nodename_match.group(1)
        features = feature_match.group(1)
        
        cabinet = extract_cabinet_from_features(features)
        if not cabinet:
            continue
        
        # Expand node ranges
        expanded_names = expand_nodename_range(nodename_pattern)
        
        for nodename in expanded_names:
            # Extract numeric portion
            m = re.match(r'nid0*(\d+)', nodename)
            if not m:
                continue
            
            nodenum = int(m.group(1))
            
            nodes.append({
                'nodename': nodename,
                'nodenum': nodenum,
                'cabinet': cabinet,
                'features': features,
            })
    
    return sorted(nodes, key=lambda n: n['nodenum'])


# ---------------------------------------------------------------------------
# LUMI-G switch assignment (corrected topology)
# ---------------------------------------------------------------------------

def lumi_g_switches(nodenum: int, cabinet: str, cabinet_base_nodenum: int) -> list[str]:
    """
    Assign NICs to switches within a LUMI-G dragonfly group (cabinet).
    
    Each cabinet (dragonfly group) has 32 64-port Rosetta switches (s0-s31).
    Each switch uses 16 ports to connect to node NICs.
    
    Topology (as documented):
    - Number nodes within each group from 0 (node0 is first in the group)
    - NICs 0,1 of nodes {0,2,4,6,8,12,14} share one switch
    - NICs 2,3 of the same nodes share a different switch
    - Odd-numbered nodes use different switches than even-numbered nodes
    - Even and odd nodes never share a switch (always requires hop)
    
    Switch assignment pattern:
    - Even nodes (0,2,4,6,8,12,14): use switches based on NIC pair
    - Odd nodes (1,3,5,7,9,13,15): use different switches
    - Pattern repeats across the group
    
    Args:
        nodenum: absolute node number (e.g., 5620)
        cabinet: cabinet ID (e.g., "x1105")
        cabinet_base_nodenum: first node number in this cabinet
    """
    # Position within cabinet (0-based)
    node_pos = nodenum - cabinet_base_nodenum
    
    if node_pos < 0:
        # Shouldn't happen with correct input
        return [f"{cabinet}s0"]
    
    is_even = (node_pos % 2) == 0
    
    # Within the pattern, even nodes use one set of switches, odd use another
    # Switches for even-node NICs:
    #   NICs 0,1: one switch
    #   NICs 2,3: another switch
    # Each pair of switches handles multiple even nodes
    
    if is_even:
        # Even node: NICs 0,1 go to one switch, NICs 2,3 to another
        node_group = node_pos // 2  # which "even-node pair" (0, 1, 2, 3, ...)
        
        # Distribute even nodes across switches
        # ~62 even nodes per group
        # 32 switches, each takes 2 NICs per node = 16 even nodes per switch
        # So ~4 switches per group of 16 even nodes
        
        switch_base = (node_group // 8) * 2  # which pair of switches
        
        # NICs 0,1 on switch 0 of pair, NICs 2,3 on switch 1 of pair
        return [
            f"{cabinet}s{switch_base}",      # NICs 0,1
            f"{cabinet}s{switch_base + 1}",  # NICs 2,3
        ]
    else:
        # Odd node: offset from even nodes
        node_group = (node_pos - 1) // 2
        
        switch_base = 16 + (node_group // 8) * 2  # Use upper half of switches
        
        return [
            f"{cabinet}s{switch_base}",      # NICs 0,1
            f"{cabinet}s{switch_base + 1}",  # NICs 2,3
        ]


# ---------------------------------------------------------------------------
# TOML serialisation helpers
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

def convert(nodelist_path: str) -> str:
    nodes = parse_nodelist(nodelist_path)
    
    if not nodes:
        raise ValueError("No nodes parsed from nodelist file")
    
    # Find first node number for each cabinet
    cabinet_base: dict[str, int] = {}
    for node in nodes:
        cabinet = node['cabinet']
        nodenum = node['nodenum']
        if cabinet not in cabinet_base:
            cabinet_base[cabinet] = nodenum
        else:
            cabinet_base[cabinet] = min(cabinet_base[cabinet], nodenum)
    
    # Collect all switches per cabinet, preserving insertion order
    switches_by_cabinet: dict[str, set[str]] = defaultdict(set)
    node_entries: list[dict] = []
    
    for node in nodes:
        nodename = node['nodename']
        nodenum = node['nodenum']
        cabinet = node['cabinet']
        
        # Get switches for this node (pass the cabinet base node number)
        sw_ids = lumi_g_switches(nodenum, cabinet, cabinet_base[cabinet])
        
        # Track switches
        for sw in sw_ids:
            switches_by_cabinet[cabinet].add(sw)
        
        node_entries.append({
            'nodename': nodename,
            'nodenum': nodenum,
            'cabinet': cabinet,
            'switches': sw_ids,
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
    lines.append("# Each cabinet is a Dragonfly group with 32 switches (s0-s31).")
    lines.append("# Intra-group connections are copper, inter-group are optical.")
    lines.append("")
    
    for cabinet in sorted(switches_by_cabinet.keys()):
        for sw_id in sorted(switches_by_cabinet[cabinet]):
            write_section(lines, "[[switch]]", {
                "id":    toml_str(sw_id),
                "group": toml_str(cabinet),
            })
    
    lines.append("# -- Nodes ---------------------------------------------------------")
    lines.append("# Each node has 4 NICs.")
    lines.append("# Pairs 0,1 and 2,3 may be on different switches.")
    lines.append("")
    
    # Sort by cabinet then by nodenum
    node_entries.sort(key=lambda e: (e['cabinet'], e['nodenum']))
    
    current_cabinet = None
    for entry in node_entries:
        cabinet = entry['cabinet']
        
        if cabinet != current_cabinet:
            lines.append(f"# cabinet {cabinet}")
            current_cabinet = cabinet
        
        fields = {
            "id":       toml_str(entry['nodename']),
            "switch": toml_str_list(entry['switches']),
            "group":    toml_str(entry['cabinet']),
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
    
    try:
        toml_text = convert(sys.argv[1])
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)
    
    if len(sys.argv) >= 3:
        with open(sys.argv[2], "w") as f:
            f.write(toml_text)
        print(f"Written to {sys.argv[2]}", file=sys.stderr)
    else:
        print(toml_text)


if __name__ == "__main__":
    main()