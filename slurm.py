import subprocess
import os
import sys
import time
from pathlib import Path

# --- Path Setup ---
sys.path.append(str(Path(__file__).parent.parent))
import job_placer as jp 

# --- TOPOLOGY Configuration ---
NUM_NODES = 8               # Total nodes for the Diamond
# Constraints: [(count, distance, parent_level), ...]
# This asks for 4 nodes at L1 distance and another 4 nodes at L1 distance
TOPOLOGY_CONSTRAINTS = [
    (3, 2.0, 1), 
    (4, 2.0, 1)
]

LEONARDO_IR_FILE = "../leo.txt" 
PARTITION = 'boost_usr_prod'
ACCOUNT = 'IscrC_OMG-25'
RETRY_DELAY = 10 

def get_idle_node_ids() -> list:
    try:
        cmd = ["sinfo", "-h", "-p", PARTITION, "-t", "idle", "-o", "%n"]
        res = subprocess.run(cmd, capture_output=True, text=True, check=True)
        return [n.strip() for n in res.stdout.split('\n') if n.strip()]
    except Exception as e:
        print(f"Error fetching sinfo: {e}")
        return []

def main():
    print(f"--- RUNNING TOPOLOGY QUERY ({NUM_NODES} NODES) ---")
    # Initialize your PyO3 Tool
    builder = jp.TopologyQueryBuilder("leonardo", LEONARDO_IR_FILE)
    
    while True:
        idle_nodes = get_idle_node_ids()
        
        if len(idle_nodes) < NUM_NODES:
            print(f"[{time.strftime('%H:%M:%S')}] Only {len(idle_nodes)} nodes idle. Need {NUM_NODES}. Waiting...")
        else:
            # Update Rust engine with current idle state
            builder.filter_by_ids(idle_nodes)
            
            found_nodelist = None

            # Exhaustive search: try every idle node as the 'Diamond' anchor
            print(f"[{time.strftime('%H:%M:%S')}] Probing {len(idle_nodes)} anchors for TOPOLOGY...")
            
            for anchor in idle_nodes:
                try:
                    # Calling your Rust shared_parent logic with multiple tuples
                    result = builder.get_nodelist_distances_shared_parent(anchor, TOPOLOGY_CONSTRAINTS)
                    if result and len(result) == NUM_NODES:
                        found_nodelist = result
                        break
                except Exception:
                    continue 

            if found_nodelist:
                print(f"✅ TOPOLOGY FOUND! Nodes: {found_nodelist}")
                
                node_string = ",".join(found_nodelist)
                srun_cmd = [
                    "srun",
                    "--partition", PARTITION,
                    "--account", ACCOUNT,
                    "--nodes", str(NUM_NODES),
                    "--ntasks", str(NUM_NODES),
                    "--nodelist", node_string,
                    "bash", "-c", 'echo "hello from node: $(hostname) | TOPOLOGY Member"'
                ]
                
                subprocess.run(srun_cmd, check=True)
                break 
            else:
                print(f"❌ TOPOLOGY configuration not available in current idle set. Retrying...")

        time.sleep(RETRY_DELAY)

if __name__ == "__main__":
    main()