# JobPlacer - Quick Start

A Rust tool for topology-aware node selection on HPC systems (SLURM).

## 1. Installation

Compile the project to generate the optimized binary:

```bash
cargo build --release
```

The binary is located at `./target/release/job_placer`.

---

## 2. Configuration (`query.json`)

Define your topology requirements in a JSON file. This allows you to request nodes based on network hops (distance) and grouping.

```json
{
  "constraints": [
    {
      "type": "NodesAtDistance",
      "count": 4,
      "distance": 2.0,
      "reference": "First"
    }
  ]
}

```

---

## 3. SLURM Integration

JobPlacer is designed to run **inside** an allocation. It reads the nodes Slurm gave you, finds the most efficient subset, and exports them.

### Workflow Example (`submit.sbatch`)

```bash
#!/bin/bash
#SBATCH -N 16               # 1. Request a large pool of nodes
#SBATCH -A <your_account>
#SBATCH -p <partition>

# 2. Run JobPlacer to find the best 4 nodes within the 16 allocated
# We capture the ::RESULT:: tag from the output
SELECTED_NODES=$(./target/release/job_placer query.json | grep '::RESULT::' | cut -d':' -f3)

if [ -z "$SELECTED_NODES" ]; then
    echo "❌ Topology search failed. No matching nodes found."
    exit 1
fi

echo "✅ Running application on: $SELECTED_NODES"

# 3. Use 'srun -w' to launch your task ONLY on the optimized subset
srun -N 4 -w "$SELECTED_NODES" --exclusive ./your_mpi_app

```

---

## 4. Output Logs

When you check your Slurm output (`cat topo_*.out`), you will see:

| Result | Meaning |
| --- | --- |
| `✅ SUCCESS! ...` | The tool found nodes matching your JSON criteria. |
| `❌ ERROR: ...` | No subset of your allocation satisfies the distance/parent constraints. |
| `Error("trailing comma"...)` | Your `query.json` has a syntax error (likely an extra comma). |

**Tip:** If it fails to find nodes, try increasing the number of nodes requested in your `#SBATCH -N` header to give the tool a larger search space.
