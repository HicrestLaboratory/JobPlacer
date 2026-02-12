#!/usr/bin/env python3
import os
import sys
from placer.nodelist_generator import LeonardoNodelistGenerator

# Configuration
TOPOLOGY_FILE = "../leo.txt"
ANCHOR_NODE = "lrdn4707"

def run_test(test_id, generator, nodes):
    filename = f"test{test_id}.svg"
    print(f"\n--- TEST {test_id} ---")
    
    if not nodes:
        print(f"  [!] Solver returned 0 nodes. Skipping.")
        return False

    try:
        builder = generator.query_builder
        # Calling your Rust method: visualize_topology(Vec<String>, String)
        builder.visualize_topology(nodes, filename)
        
        if os.path.exists(filename):
            print(f"  [✓] Nodes: {len(nodes)} | Saved as: {filename}")
            return True
        else:
            print(f"  [✗] Error: Rust failed to generate {filename}")
            return False
    except Exception as e:
        print(f"  [!] Exception: {e}")
        return False

def main():
    if not os.path.exists(TOPOLOGY_FILE):
        print(f"Error: {TOPOLOGY_FILE} not found.")
        sys.exit(1)

    gen = LeonardoNodelistGenerator(TOPOLOGY_FILE)

    # 1. Complex Mult-Switch / Spine tests
    nodes_1 = gen.get_nodelist(
        anchor=ANCHOR_NODE, 
        distances=[(4, 2.0, 1), (4, 4.0, 1)], 
        shared_parent=True
    )
    run_test(1, gen, nodes_1)

    nodes_2 = gen.get_nodelist(
        anchor=ANCHOR_NODE,
        distances=[(2, 2.0, 1), (2, 5.0, 2)],
        shared_parent=True
    )
    run_test(2, gen, nodes_2)

    nodes_3 = gen.get_nodelist(
        anchor=ANCHOR_NODE,
        distances=[(2, 2.0, 1), (2, 4.0, 1), (2, 4.0, 2)],
        shared_parent=True
    )
    run_test(3, gen, nodes_3)

    # 2. Local / Validation tests
    nodes_4 = gen.get_nodelist(anchor=ANCHOR_NODE, distances=[(7, 2.0)])
    run_test(4, gen, nodes_4)

    nodes_5 = gen.get_nodelist(
        anchor=ANCHOR_NODE, 
        distances=[(7, 2.0, 1), (8, 5.0, 1)], 
        shared_parent=True
    )
    run_test(5, gen, nodes_5)

if __name__ == "__main__":
    main()