# JobPlacer - Quick Start

A Rust tool for topology-aware node selection on HPC clusters (SLURM).

## 1. Installation

Compile the project in release mode to generate the optimized binary:

```bash
cargo build --release

```

The binary will be created at: `./target/release/job_placer`

---

## 2. Usage with SLURM

To run a job using intelligent node selection, follow these steps.

### A. Define the Topology (`query.json`)

Create a JSON file describing your node requirements (e.g., "4 nodes close together").

**Example `query.json`:**

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

### B. Submit the Script (`test.sbatch`)

Ensure your `test.sbatch` script calls the binary with the JSON file as an argument.

**Example command inside the script:**

```bash
# ... sbatch headers ...
./target/release/job_placer query.json

```

Submit the job:

```bash
sbatch test.sbatch

```

### C. Check the Output

Once the job finishes, verify which nodes were selected in the log file:

```bash
cat topo_*.out

```

*You should see output like:* `✅ SUCCESS! Optimal Nodes Selected: lrdn0001,lrdn0002...`
or if it fails you should see output like: ❌ ERROR: Topology search failed. No matching nodes found.
