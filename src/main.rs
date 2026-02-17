use std::env;
use std::fs;
use std::process::Command;
use serde::Deserialize;
use job_placer::parsers::leonardo;
use job_placer::ir::id::Id;
use job_placer::query::{TopologyQuery, Constraint, ReferencePoint, DistanceGroup};

// --- JSON Input Structure ---
#[derive(Deserialize, Debug)]
struct QueryInput {
    constraints: Vec<ConstraintInput>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")] // Distinguish by "type" field
enum ConstraintInput {
    NodesAtDistance {
        count: usize,
        distance: f64,
        reference: String, // "First" or "Last"
    },
    NodesAtDistanceWithSharedParent {
        count: usize,
        distance: f64,
        reference: String,
        parent_level: usize,
    },
    DistanceGroup {
        reference: String,
        groups: Vec<DistanceGroupInput>,
    },
}

#[derive(Deserialize, Debug)]
struct DistanceGroupInput {
    count: usize,
    distance: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Get CLI Argument (JSON file path)
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <query.json>", args[0]);
        std::process::exit(1);
    }
    let json_path = &args[1];

    println!("--- 🦀 Rust Job Placer ---");
    println!("Loading query from: {}", json_path);

    // 2. Parse JSON
    let json_content = fs::read_to_string(json_path)?;
    let query_input: QueryInput = serde_json::from_str(&json_content)?;

    // 3. Get Allocation (SLURM)
    let allocated_hostnames = get_slurm_nodes()?;
    // Mock for local testing if empty
    let allocated_hostnames = if allocated_hostnames.is_empty() {
        println!("⚠️  Warning: Not inside Slurm. Using mock data.");
        vec!["lrdn0247".to_string(), "lrdn0248".to_string(), "lrdn0249".to_string(), "lrdn0314".to_string()]
    } else {
        allocated_hostnames
    };
    
    let allocated_ids: Vec<Id> = allocated_hostnames.iter().map(|n| Id::from(n.as_str())).collect();
    println!("✓ Allocation: {} nodes", allocated_ids.len());

    // 4. Load & Filter Topology
    let full_ir = leonardo::from_file("../leo.txt");
    let my_allocation_ir = full_ir.filter_with_topology(&allocated_ids);

    // 5. Build Query from JSON
    let mut query = TopologyQuery::new();
    
    for c in query_input.constraints {
        let constraint = match c {
            ConstraintInput::NodesAtDistance { count, distance, reference } => {
                Constraint::NodesAtDistance {
                    count,
                    distance,
                    reference: parse_ref(&reference),
                }
            },
            ConstraintInput::NodesAtDistanceWithSharedParent { count, distance, reference, parent_level } => {
                Constraint::NodesAtDistanceWithSharedParent {
                    count,
                    distance,
                    reference: parse_ref(&reference),
                    parent_level,
                }
            },
            ConstraintInput::DistanceGroup { reference, groups } => {
                let parsed_groups = groups.iter().map(|g| DistanceGroup {
                    count: g.count,
                    distance: g.distance
                }).collect();
                
                Constraint::DistanceGroup {
                    reference: parse_ref(&reference),
                    groups: parsed_groups,
                }
            }
        };
        query = query.with_constraint(constraint);
    }

    // 6. Execute Search
    let mut success = false;
    for anchor in &allocated_ids {
        if let Ok(selected_nodes) = query.execute_from(&my_allocation_ir, anchor.clone()) {
            let result_str = selected_nodes.iter().map(|id| id.0.clone()).collect::<Vec<_>>().join(",");
            println!("   Nodes: {}", result_str); 
            println!("::RESULT::{}", result_str); // Machine readable tag
            success = true;
            break;
        }
    }

    if !success {
        eprintln!("❌ Topology search failed.");
        std::process::exit(1);
    }

    Ok(())
}

fn parse_ref(s: &str) -> ReferencePoint {
    match s.to_lowercase().as_str() {
        "last" => ReferencePoint::Last,
        _ => ReferencePoint::First,
    }
}

fn get_slurm_nodes() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let nodelist_env = match env::var("SLURM_JOB_NODELIST") {
        Ok(val) => val,
        Err(_) => return Ok(vec![]),
    };
    let output = Command::new("scontrol").arg("show").arg("hostnames").arg(nodelist_env).output()?;
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
}
