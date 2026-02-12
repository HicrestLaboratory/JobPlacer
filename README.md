# JobPlacer

**Intelligent job placement for HPC clusters using topology-aware node selection.**

JobPlacer is a Rust library with Python bindings that enables precise control over node placement in HPC job scheduling. It uses graph-based topology analysis to select compute nodes based on their network distances and hierarchical relationships.

---

## Table of Contents

- [Overview](#overview)
- [Installation](#installation)
  - [Prerequisites](#prerequisites)
  - [Quick Install](#quick-install)
  - [Manual Installation](#manual-installation)
  - [Verify Installation](#verify-installation)
  - [Installing on Leonardo](#installing-on-leonardo)
- [Core Concepts](#core-concepts)
- [Usage](#usage)
- [Examples](#examples)

---

## Installation

### Prerequisites

Before installing JobPlacer, ensure you have:

1. **Rust** (1.70 or later)
```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source $HOME/.cargo/env
```

2. **Python** (3.8 or later)
```bash
   python3 --version  # Should be 3.8+
```

3. **pip** (Python package installer)
```bash
   python3 -m pip --version
```

### Quick Install

The easiest way to install JobPlacer:
```bash
# Clone the repository
git clone https://github.com/your-org/JobPlacer.git
cd JobPlacer

# Run the installation script
chmod +x install.sh
./install.sh
```

The installation script will:
1. ✅ Check for Rust and Python
2. ✅ Install Python dependencies (maturin, setuptools-rust)
3. ✅ Build the Rust extension with optimizations
4. ✅ Install the Python module
5. ✅ Verify the installation

### Verify Installation

After installation, test that everything works:
```bash
python3 << EOF
import job_placer

# Check module loaded
print("✓ job_placer module loaded")

# Check TopologyQueryBuilder available
qb = job_placer.TopologyQueryBuilder
print("✓ TopologyQueryBuilder available")

print("\n🎉 JobPlacer is ready to use!")
EOF
```

## Quick Start

After installation, try this simple example:
```python
from nodelists_generator_rust import RustNodelistGenerator

# Initialize with Leonardo topology
generator = RustNodelistGenerator("leo.txt")

# Get all available compute nodes
nodes = generator.get_compute_nodes()
print(f"Available compute nodes: {len(nodes)}")

# Generate a nodelist: 2 nodes at distance 2, 2 nodes at distance 4
nodelist = generator.get_nodelist_custom_distances(
    partition="boost_usr_prod",
    total_nodes=5,  # 1 anchor + 4 others
    distances=[
        (2, 2.0),  # 2 nodes at distance 2
        (2, 4.0),  # 2 nodes at distance 4
    ],
    shared_parent=False
)

print(f"Generated nodelist: {nodelist}")
# Output: lrdn[4707-4709,5001-5002]
```

# Core Concepts

## Network Distance

**Distance** is defined as the number of hops (edges) in the network graph between two compute nodes.

Each physical link counts as **1 hop**:

- Compute ↔ L1 switch  
- L1 ↔ L2 switch  
- L2 ↔ L3 switch  

Distance is purely graph-based. It does not depend on bandwidth or latency — only on topology.

---

# Reference Topology

All examples use the following hierarchical network:

```
                       l3sw1
                     /       \
                 l2sw1       l2sw2
                /     \     /     \
            l1sw1   l1sw2 l1sw3  l1sw4
           /  \     /  \   /  \    /  \
         cn1 cn2  cn3 cn4 cn5 cn6 cn7 cn8
```

### Hierarchy Levels

- **Level 1** – Compute nodes  
- **Level 2** – L1 switches (rack level)  
- **Level 3** – L2 switches (aggregation level)  
- **Level 4** – L3 switch (top-level core)  

Anchor node: **cn1**

---

## Distances from cn1

- **2 hops**
  - cn2  
  `cn1 → l1sw1 → cn2`

- **4 hops**
  - cn3  
  `cn1 → l1sw1 → l2sw1 → l1sw2 → cn3`
  - cn4  
  same length = 4 hops

- **6 hops**
  - cn5  
  `cn1 → l1sw1 → l2sw1 → l3sw1 → l2sw2 → l1sw3 → cn5`
  - cn6  
  same length = 6 hops
  - cn7  
  same length = 6 hops
  - cn8  
  same length = 6 hops

---

## Distance Rule

The distance between two nodes depends on their **lowest common ancestor** in the hierarchy:

- Same L1 → 2 hops  
- Same L2 (different L1) → 4 hops  
- Same L3 only → 6 hops  

Distance from the anchor does **not** guarantee proximity among selected nodes.

---

# Shared Parent Constraints

A **shared parent constraint** requires selected nodes to share the same ancestor at a specified hierarchy level.

- Distance controls **anchor locality**  
- Shared parent controls **group locality**

Both are independent.

---

# Example: Select 2 Nodes at Distance 6 from cn1

Candidates:

- cn5
- cn6
- cn7
- cn8

---

## Case 1 — Without Shared Parent Constraint

Possible selection:

- cn5
- cn7

```
                       l3sw1
                     /      \
                 l2sw1      l2sw2
                   |         /  \
                 l1sw1   l1sw3  l1sw4
                   |        |      |
                  cn1      cn5*   cn7*
```

(\* = selected)

Properties:

- Both are 6 hops from cn1  
- They are 4 hops from each other  
- They do not share the same L1  
- No locality guarantee  

Distance requirement is satisfied.  
Group locality is uncontrolled.

---

## Case 2 — Shared Parent at Level 1 (L1)

Query:

> Select 2 nodes at distance 6 that share the same L1

Possible selection:

- cn5-cn6

```
                       l3sw1
                     /       \
                 l2sw1       l2sw2
                   |           |    
                l1sw1        l1sw3  
                  |           /  \   
                 cn1        cn5* cn6* 
```

Properties:

- Both are 6 hops from cn1  
- They are 2 hops from each other  
- They share the same L1 (l1sw3)

