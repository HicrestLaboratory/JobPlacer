use std::env;
use std::process::Command;
use job_placer::parsers::leonardo;
use job_placer::ir::id::Id;
use job_placer::query::{TopologyQuery, Constraint, ReferencePoint, DistanceGroup};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- 🚀 Rust Job Placer: Inside Allocation ---");

    // 1. GET ALLOCATED NODES (From Slurm)
    // We get the list of nodes Slurm gave us (e.g., 16 nodes)
    let allocated_hostnames = get_slurm_nodes()?;
    println!("✓ Found allocation of {} nodes", allocated_hostnames.len());

    if allocated_hostnames.is_empty() {
        return Err("No nodes found. Are you running inside a Slurm job?".into());
    }

    // Convert strings to your Crate's ID type
    let allocated_ids: Vec<Id> = allocated_hostnames
        .iter()
        .map(|name| Id::from(name.as_str()))
        .collect();

    // 2. LOAD FULL TOPOLOGY
    // Load the massive graph of the whole cluster
    println!("... Loading full topology (leo.txt)");
    let full_ir = leonardo::from_file("../leo.txt");

    // 3. FILTER TOPOLOGY
    // Create a mini-graph containing ONLY the nodes we own.
    // This prevents the query from selecting nodes that belong to other users.
    println!("... Filtering topology to allocated nodes only");
    let my_allocation_ir = full_ir.filter_with_topology(&allocated_ids);

    // 4. DEFINE YOUR QUERY
    // Example: "I need 4 nodes: 1 anchor + 3 others nearby"
    let target_node_count = 4; 
    
    // Safety check: Do we have enough nodes?
    if allocated_ids.len() < target_node_count {
        return Err(format!("Allocation too small! Need {}, got {}", target_node_count, allocated_ids.len()).into());
    }

    let query = TopologyQuery::new()
        .with_constraint(Constraint::DistanceGroup {
            reference: ReferencePoint::First,
            groups: vec![
                // Example: Find 3 neighbors at distance 4.0 (same switch)
                DistanceGroup { count: 3, distance: 4.0 }, 
            ],
        });

    // 5. EXECUTE QUERY
    // We try to find the shape. Since we don't know which node is the "best" anchor
    // in our allocation, we iterate through our allocated nodes and try each one as the start.
    
    let mut final_selection: Option<Vec<Id>> = None;

    for anchor in &allocated_ids {
        match query.execute_from(&my_allocation_ir, anchor.clone()) {
            Ok(nodes) => {
                // We found a valid shape!
                final_selection = Some(nodes);
                break; // Stop searching
            },
            Err(_) => {
                // This node couldn't serve as an anchor for this shape. 
                // Continue to the next one.
                continue;
            }
        }
    }

    // 6. OUTPUT RESULT
    match final_selection {
        Some(nodes) => {
            println!("\n✅ SUCCESS! Found a matching topology subset:");
            let node_str: Vec<String> = nodes.iter().map(|id| id.0.clone()).collect();
            let result_str = node_str.join(",");
            println!("   Nodes: {}", result_str);
            
            // OPTIONAL: Write to a file so a bash script can pick it up
            // std::fs::write("selected_nodes.txt", result_str)?;
        },
        None => {
            eprintln!("\n❌ FAILURE: Allocated nodes do not contain the requested topology.");
            return Err("Topology constraint unsatisfiable in current allocation".into());
        }
    }

    Ok(())
}

/// Helper: Reads SLURM_JOB_NODELIST and expands it using `scontrol`
fn get_slurm_nodes() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // 1. Read env var
    let nodelist_env = env::var("SLURM_JOB_NODELIST")
        .map_err(|_| "SLURM_JOB_NODELIST not set")?;

    // 2. Run `scontrol show hostnames <list>`
    // This handles the compressed format like "lrdn[0001-0004]" -> "lrdn0001", "lrdn0002"...
    let output = Command::new("scontrol")
        .arg("show")
        .arg("hostnames")
        .arg(nodelist_env)
        .output()?;

    if !output.status.success() {
        return Err("Failed to run scontrol".into());
    }

    let stdout = String::from_utf8(output.stdout)?;
    
    // 3. Split by newline into a Vec
    let nodes: Vec<String> = stdout
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    Ok(nodes)
}