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
            "distance": 2.0,
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
