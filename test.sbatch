#!/bin/bash
#SBATCH -A IscrC_OMG-25        # Replace with your actual account (e.g., tra24_...)
#SBATCH -p boost_usr_prod        # Partition
#SBATCH --time=00:10:00          # Max runtime
#SBATCH -N 16                    # 1. Allocate 16 nodes (search space)
#SBATCH --ntasks-per-node=1
#SBATCH --job-name=RustTopo      # Job name
#SBATCH --output=topo_%j.out     # Output log file

# ==============================================================================
# STEP 1: Run Topology Selection (Rust Binary)
# ==============================================================================
echo "--- 🚀 Searching for optimal topology within $SLURM_JOB_NUM_NODES nodes ---"

cat <<EOF > my_query.json
{
  "constraints": [
    {
      "type": "NodesAtDistance",
      "count": 3,
      "distance": 4.0,
      "reference": "First"
    }
  ]
}
EOF

echo $SLURM_JOB_NODELIST

# 1. Run the Rust tool and capture ALL output into a variable
#    (Assumes binary is at ./target/release/job_placer)
TOPO_OUTPUT=$(./target/release/job_placer my_query.json)

# 2. Extract the list of nodes
#    We look for the line "   Nodes: lrdn01,lrdn02..." and grab the second word.
SELECTED_NODES=$(echo "$TOPO_OUTPUT" | grep "Nodes:" | awk '{print $2}')

# ==============================================================================
# STEP 2: Verify & Execute
# ==============================================================================

# Check if the variable is empty
if [ -z "$SELECTED_NODES" ]; then
    echo "❌ ERROR: Topology search failed. No matching nodes found."
    echo "DEBUG: Full Output from Rust tool:"
    echo "$TOPO_OUTPUT"
    exit 1
fi

echo "✅ SUCCESS! Optimal Nodes Selected: $SELECTED_NODES"
echo "--- 🏁 Launching Application on Subset ---"

# 3. Launch the actual job ONLY on the selected nodes
#    -w : Forces Slurm to run on this specific list
#    -N : Must match the number of nodes you found (e.g., 4)
#    --exclusive : Ensures pure dedicated access

srun -N 4 -w "$SELECTED_NODES" --exclusive \
    bash -c "echo I am running on optimized node: \$(hostname)"

echo "--- Done ---"
rm my_query.json
