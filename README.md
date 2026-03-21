# JobPlacer

A Rust tool for topology-aware node selection on HPC systems (SLURM).

## Installation

Compile the project to generate the optimized binary:

```bash
cargo build --release
```

The binary is located at `./target/release/job_placer`.

---

## 🔍 How do Queries Work

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
- `NodesAtDistanceWithSharedParent`: Selects nodes at the specified distance that also share the same parent at a given topology level. Additional required field: `parent_level`.

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

---

## SLURM Integration

JobPlacer is designed to run **inside** an allocation. It reads the nodes SLURM gave you, finds the most efficient subset, and exports them.

### Usage Example (`submit.sbatch`)

```bash
#!/bin/bash
#SBATCH -A IscrC_OMG-25
#SBATCH -p boost_usr_prod
#SBATCH --time=00:01:00
#SBATCH -N 8
#SBATCH --ntasks-per-node=1
#SBATCH --job-name=RustTopo
#SBATCH --output=topo_%j.out

# ==============================================================================
# STEP 1: Run Topology Selection (Rust Binary)
# ==============================================================================
echo "--- Searching for optimal topology within $SLURM_JOB_NUM_NODES nodes ---"
echo "Allocated nodes: $SLURM_JOB_NODELIST"

# Run the Rust tool and capture ALL output into a variable
TOPO_OUTPUT=$(./target/release/job_placer -s leonardo --topology-scontrol <(cat <<EOF
{
    "constraints": [
        {
            "type": "NodesAtDistance",
            "count": 1,
            "distance": 2,
            "reference": "First"
        }
    ]
}
EOF
))
TOPO_EXIT_CODE=$?

echo "Tool exit code: $TOPO_EXIT_CODE"
echo "Tool output: $TOPO_OUTPUT"

# ==============================================================================
# STEP 2: Verify & Execute
# ==============================================================================

# Check if the command failed
if [ $TOPO_EXIT_CODE -ne 0 ]; then
    echo "❌ ERROR: Topology search failed with exit code $TOPO_EXIT_CODE."
    exit $TOPO_EXIT_CODE
fi

SELECTED_NODES="$TOPO_OUTPUT"

echo "✅ SUCCESS! Selected Nodes: $SELECTED_NODES"
echo "--- Launching Application on Selected Nodes ---"

# Launch the actual job ONLY on the selected nodes
#    -w : Forces Slurm to run on this specific list
#    -N : Must match the number of nodes you found
#    --exclusive : Ensures pure dedicated access

srun -N 2 -w "$SELECTED_NODES" --exclusive \
    bash -c "echo I am running on a selected node: \$(hostname)"

echo "--- Done ---"

```

---

## 4. Output Logs

When you check your Slurm output (`cat topo_*.out`), you will see:

| Result | Meaning |
| --- | --- |
| `✅ SUCCESS! ...` | The tool found nodes matching your JSON criteria. |
| `❌ ERROR: ...` | No subset of your allocation satisfies the distance/parent constraints. |

**Tip:** If it fails to find nodes, try increasing the number of nodes requested in your `#SBATCH -N` header to give the tool a larger search space.


## Concurrent Jobs placement

Placement classes define how a job's nodes are mapped onto the Dragonfly topology:

| Class | Description |
|-------|-------------|
| `intra-l1` | All nodes on a single switch |
| `intra-group` | Nodes spread across switches within one Dragonfly group |
| `inter-group` | Nodes span multiple Dragonfly groups |
| `intra-group-same-l1-2/4` | Nodes grouped in blocks of 2 or 4, each block on the same switch, all within one group |
| `inter-group-same-l1-2/4` | Same block structure, but blocks distributed across multiple groups |



### Manual Debug Commands

```bash
# Build
cargo build 
cargo build --release

# Visualize full graph (this will generate <system>_topo.svg)
./target/debug/job_placer_viz -v --system leonardo -F leonardo_topo.txt --sinfo-file leonardo_sinfo.txt -p boost_usr_prod
./target/debug/job_placer_viz -v --system alps -f systems/ALPS.toml -F alps_topo.txt --sinfo-file alps_sinfo.txt -p normal
./target/debug/job_placer_viz -v --system jupiter -F jupiter_topo.txt --sinfo-file jupiter_sinfo.txt -p booster

./target/debug/job_placer_viz -v --system jupiter -F jupiter_topo.txt --nodelist "jpbo-001-[01-48],jpbo-002-[01-48],jpbo-003-[01-48],jpbo-092-[01-48],jpbo-093-[01-48],jpbo-094-[01-48],jpbo-095-[01-48],jpbo-101-[01-48],jpbo-102-[01-48],jpbo-103-[01-48],jpbo-104-[01-48],jpbo-105-[01-48]" --sinfo-file jupiter_sinfo.txt --out-svg topo_jupiter_nodelist.svg

# Placement classes
./target/debug/job_placer_placement_classes -v --system leonardo -F leonardo_topo.txt --sinfo-file leonardo_sinfo.txt -p boost_usr_prod --out-svg placement_leonardo.svg --seed 0 <(cat example/placements/test.json)
./target/debug/job_placer_placement_classes -v --system jupiter -F jupiter_topo.txt --sinfo-file jupiter_sinfo.txt --nodelist "jpbo-001-[01-48],jpbo-002-[01-48],jpbo-003-[01-48],jpbo-092-[01-48],jpbo-093-[01-48],jpbo-094-[01-48],jpbo-095-[01-48],jpbo-101-[01-48],jpbo-102-[01-48],jpbo-103-[01-48],jpbo-104-[01-48],jpbo-105-[01-48]" --out-svg placement_jupiter.svg --seed 0 <(cat example/placements/test.json)
./target/debug/job_placer_placement_classes -v --system alps -f systems/ALPS.toml -F alps_topo.txt --sinfo-file alps_sinfo.txt --nodelist "nid[005449-005507,005510-005559],nid[005896-006003],nid[006458-006567]" --out-svg placement_alps.svg --seed 0 <(cat example/placements/test.json)
```