# JobPlacer

JobPlacer is a CLI tool for topology-aware node selection on HPC systems. It has built-in support for the topologies of Leonardo, JUPITER, LUMI, and Alps (Daint and Clariden), modeling the hierarchy of compute nodes, switches, and Dragonfly(+) groups.  

The tool is easily extensible to new systems by providing a topology map in TOML format. It can query, analyze and visualize how the nodes assigned to a job are distributed across the system topology, making it useful both for forcing node placement before job submission and for analyzing allocations after execution.  

JobPlacer supports a variety of placement strategies, including intra-L1, intra-group, inter-group, and other topology-aware placement policies.

## Installation

Compile the project to generate the optimized binaries:

```bash
cargo build --release
```

Binaries are located at `./target/release/job_placer_*`.

---

## Concurrent Jobs placement

Placement classes define how a job's nodes are mapped onto the Dragonfly topology:

| Class                       | Description |
|-----------------------------|-------------|
| `intra-l1`                  | All nodes on a single switch |
| `intra-group`               | Nodes spread across switches within one Dragonfly group |
| `inter-group`               | Nodes span multiple Dragonfly groups |
| `intra-group-same-l1-2/4`   | Nodes grouped in blocks of 2 or 4, each block on the same switch, all within one group |
| `inter-group-same-l1-2/4`   | Same block structure, but blocks distributed across multiple groups |


### Example - Single Job Placement

```bash
./target/release/job_placer_placement_classes --system leonardo -a -F leonardo_topo.txt --sinfo-file leonardo_sinfo.txt -p boost_usr_prod --out-svg placement_leonardo_single.svg --seed 0 <(cat <<EOF
{
  "intra-l1_4n": { "nodes": 4, "placement_class": "intra-l1" }
}
EOF
)
```

### Example - Concurrent Jobs Placement

```bash
./target/release/job_placer_placement_classes --system leonardo -a -F leonardo_topo.txt --sinfo-file leonardo_sinfo.txt -p boost_usr_prod --out-svg placement_leonardo_concurrent.svg --seed 0 <(cat <<EOF
{
  "inter-group_7n":           { "nodes": 7,  "placement_class": "inter-group"            },
  "intra-l1_6n":              { "nodes": 6,  "placement_class": "intra-l1"               },
  "intra-group_6n":           { "nodes": 6,  "placement_class": "intra-group"            },
  "intra-group-same-l1-2_6n": { "nodes": 6,  "placement_class": "intra-group-same-l1-2"  },
  "intra-group-same-l1-4_4n": { "nodes": 8,  "placement_class": "intra-group-same-l1-4"  },
  "inter-group-same-l1-2_6n": { "nodes": 6,  "placement_class": "inter-group-same-l1-2"  },
  "inter-group-same-l1-4_8n": { "nodes": 8,  "placement_class": "inter-group-same-l1-4"  }
}
EOF
)
```

***Note**: the files `leonardo_topo.txt` and `leonardo_sinfo.txt` are output snapshots of commands `scontrol show topology` and `sinfo` run on the Leonardo supercomputer (resp.). This is only meant for local tests and examples. If flags `-F` and `--sinfo-file` are omitted, the related information will be fetched dynamically from the system.*

---

## SLURM Integration - Example

JobPlacer is designed to run **inside** an allocation.

```bash
#!/bin/bash
#SBATCH -N 8
#SBATCH --ntasks-per-node=1
#SBATCH -A <your_account>
#SBATCH -p boost_usr_prod
#SBATCH --time=00:01:00
#SBATCH --job-name=JobPlacer
#SBATCH --output=jobplacer_%j.out

# ==============================================================================
# STEP 1: Run JobPlacer
# ==============================================================================
echo "--- Placing job within $SLURM_JOB_NUM_NODES allocated nodes ---"
echo "Allocated nodes: $SLURM_JOB_NODELIST"

# Run JobPlacer and capture output (this will automatically detect allocated nodes)
JOBPLACER_OUTPUT=$(./target/release/job_placer_placement_classes --system leonardo -p boost_usr_prod --out-svg runtime_placement_leonardo.svg --seed 0 <(cat <<EOF
{
  "intra-group_2n": { "nodes": 2, "placement_class": "intra-group" }
}
EOF
))
JOBPLACER_EXIT_CODE=$?

echo "JobPlacer exit code: $JOBPLACER_EXIT_CODE"
echo "Tool output: $JOBPLACER_OUTPUT"

# ==============================================================================
# STEP 2: Verify & Execute
# ==============================================================================

# Check if the command failed
if [ $JOBPLACER_EXIT_CODE -ne 0 ]; then
    echo "❌ ERROR: Placement failed with exit code $JOBPLACER_EXIT_CODE."
    exit $JOBPLACER_EXIT_CODE
fi

SELECTED_NODES=$(python3 -c 'import json,sys; d=json.load(sys.stdin); print(",".join(next(iter(d["placements"].values()))["nodes"]))' <<< "$JOBPLACER_OUTPUT")

echo "✅ SUCCESS! Selected Nodes: $SELECTED_NODES"
echo "--- Launching Application on Selected Nodes ---"

# Launch the actual job ONLY on the selected nodes
#    -w : Forces Slurm to run on this specific list
#    -N : Must match the number of nodes you found

srun -N 2 -w "$SELECTED_NODES" \
    bash -c "echo I am running on selected same dragonfly+ group nodes: \$(hostname)"

echo "--- Done ---"
```

---

# Python CLI Wrapper

The `cli_wrapper.py` script provides a CLI wrapper for the `job_placer_placement_classes` binary.  
This allows for a programmatic use of the tool. For example:

```python
from pprint import pprint
from cli_wrapper import JobPlacer

placer = JobPlacer(
  system='leonardo',
  topology_file='leonardo_topo.txt',
  sinfo_file='leonardo_sinfo.txt',
  all_nodes=True,
  verbose=False,
)

pprint(placer.place({
  "inter-group_7n":           { "nodes": 7,  "placement_class": "inter-group"            },
  "intra-l1_6n":              { "nodes": 6,  "placement_class": "intra-l1"               },
  "intra-group_6n":           { "nodes": 6,  "placement_class": "intra-group"            },
  "intra-group-same-l1-2_6n": { "nodes": 6,  "placement_class": "intra-group-same-l1-2"  },
  "intra-group-same-l1-4_4n": { "nodes": 8,  "placement_class": "intra-group-same-l1-4"  },
  "inter-group-same-l1-2_6n": { "nodes": 6,  "placement_class": "inter-group-same-l1-2"  },
  "inter-group-same-l1-4_8n": { "nodes": 8,  "placement_class": "inter-group-same-l1-4"  }
}))
```

<!-- ## 🔍 How do Queries Work

JobPlacer doesn't just look at a list of nodes; it evaluates them based on a **Starting Node** (the anchor) and how they connect through the network tree.

### 1. The Starting Node

The anchor node is automatically selected. If you are under a SLURM allocation or if you specify an explicit nodelist, anchors will be sampled from that pool of nodes. All other nodes are then checked against this anchor to see if they meet your requirements.

### 2. Distance (Number of Hops)
Distance is simply the number of "steps" or "hops" between the Anchor and another node.

Define your topology requirements in a JSON file.  
Constraints allow selecting nodes based on **network distance (hops)**.

### 3. Shared Parent (Common Group)
This is a rule that forces the search to stay within a specific "branch" or "box" in the network tree.

* **The Constraint:** When you request nodes at a certain distance, you can also require that they all share the same **Parent**.
* **Flexible Levels:** This parent can be a small local switch or a large group switch.

| Feature | Definition | Simple Meaning |
| :--- | :--- | :--- |
| **Starting Node** | The Anchor | The central point used to measure everyone else. |
| **Distance** | Number of Hops | How many steps away from the anchor is this node? |
| **Shared Parent** | Common Group | Do these nodes share the same "ancestor" switch? |

### JSON Query Structure

The following example query, starting from an **anchor** node, is searching for 2 nodes on the same L1 switch (distance 2: Anchor -> L1 switch -> TargetNode). The second constraint looks for an additional node ad distance 4 (Anchor -> L1 switch #1 -> L2 switch -> L1 switch #2 -> TargetNode).

```json
{
  "constraints": [
    {
      "type": "NodesAtDistance",
      "count": 2,
      "distance": 2,
      "reference": "First"
    },
    {
      "type": "NodesAtDistance",
      "count": 1,
      "distance": 4,
      "reference": "First"
    }
  ]
}
```

**Required Constraint Fields**
- `type`: Constraint type (see below).
- `count`: Number of nodes to select.
- `distance`: Required network distance (in hops).
- `reference`: Reference node for distance calculation (e.g., "First").

**Constraint Types**
- `NodesAtDistance`: Selects nodes at the specified distance from the anchor node.
- `NodesAtDistanceWithSharedParent`: Selects nodes at the specified distance that also share the same parent at a given topology level. Additional required field: `parent_level`. -->

<!-- TODO make example {
  "constraints": [
    {
      "type": "NodesAtDistanceWithSharedParent",
      "count": 3,
      "distance": 5,
      "parent_level": 2
    }
  ]
} -->


<!-- ---

## 4. Output Logs

When you check your Slurm output (`cat topo_*.out`), you will see:

| Result | Meaning |
| --- | --- |
| `✅ SUCCESS! ...` | The tool found nodes matching your JSON criteria. |
| `❌ ERROR: ...` | No subset of your allocation satisfies the distance/parent constraints. |

**Tip:** If it fails to find nodes, try increasing the number of nodes requested in your `#SBATCH -N` header to give the tool a larger search space. -->


<!-- 
### Manual Debug Commands

```bash
# Build
cargo build 
cargo build --release

# Visualize full graph (this will generate <system>_topo.svg)
./target/debug/job_placer_viz -v --system leonardo -a -F leonardo_topo.txt --sinfo-file leonardo_sinfo.txt -p boost_usr_prod
./target/debug/job_placer_viz -v --system alps -a -f systems/ALPS.toml -F alps_topo.txt --sinfo-file alps_sinfo.txt -p normal
./target/debug/job_placer_viz -v --system jupiter -a -F jupiter_topo.txt --sinfo-file jupiter_sinfo.txt -p booster

./target/debug/job_placer_viz -v --system jupiter -F jupiter_topo.txt --nodelist "jpbo-001-[01-48],jpbo-002-[01-48],jpbo-003-[01-48],jpbo-092-[01-48],jpbo-093-[01-48],jpbo-094-[01-48],jpbo-095-[01-48],jpbo-101-[01-48],jpbo-102-[01-48],jpbo-103-[01-48],jpbo-104-[01-48],jpbo-105-[01-48]" --sinfo-file jupiter_sinfo.txt --out-svg topo_jupiter_nodelist.svg

# Placement classes
./target/debug/job_placer_placement_classes -v --system leonardo -a -F leonardo_topo.txt --sinfo-file leonardo_sinfo.txt -p boost_usr_prod --out-svg placement_leonardo.svg --seed 0 <(cat example/placements/test.json)
./target/debug/job_placer_placement_classes -v --system jupiter -F jupiter_topo.txt --sinfo-file jupiter_sinfo.txt --nodelist "jpbo-001-[01-48],jpbo-002-[01-48],jpbo-003-[01-48],jpbo-092-[01-48],jpbo-093-[01-48],jpbo-094-[01-48],jpbo-095-[01-48],jpbo-101-[01-48],jpbo-102-[01-48],jpbo-103-[01-48],jpbo-104-[01-48],jpbo-105-[01-48]" --out-svg placement_jupiter.svg --seed 0 <(cat example/placements/test.json)
./target/debug/job_placer_placement_classes -v --system alps -f systems/ALPS.toml -F alps_topo.txt --sinfo-file alps_sinfo.txt --nodelist "nid[005449-005507,005510-005559],nid[005896-006003],nid[006458-006567]" --out-svg placement_alps.svg --seed 0 <(cat example/placements/test.json)
``` -->