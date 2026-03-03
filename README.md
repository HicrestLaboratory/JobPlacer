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
