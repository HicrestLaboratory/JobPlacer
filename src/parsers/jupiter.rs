use std::{collections::HashMap, fs, path::Path};
use crate::ir::entity::{Entity, EntityKind};
use crate::ir::id::Id;
use crate::ir::topology_ir::TopologyIR;
use crate::parsers::run_scontrol_show_topology;
use crate::parsers::slurm::{NodeListParseError, expand_nodelist, parse_line};

// TODO move to topology
// const L2_TO_L1_PORTS: u32 = 10;

/// Parse Jupiter topology by executing `scontrol show topology`
pub fn from_scontrol() -> Result<TopologyIR, NodeListParseError> {
    parse_topology(run_scontrol_show_topology())
}

/// Parse Jupiter topology from a file (for testing)
pub fn from_file<P: AsRef<Path>>(path: P) -> Result<TopologyIR, NodeListParseError> {
    parse_topology(fs::read_to_string(path).expect("Failed to read topology file"))
}

/// Parse topology string into TopologyIR
fn parse_topology(output: String) -> Result<TopologyIR, NodeListParseError> {
    let mut ir = TopologyIR::default();

    // Merge multi-line entries
    let mut lines = Vec::new();
    let mut current = String::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        if trimmed.starts_with("SwitchName=") {
            if !current.is_empty() { lines.push(current.clone()); }
            current = trimmed.to_string();
        } else {
            current.push(' ');
            current.push_str(trimmed);
        }
    }
    if !current.is_empty() { lines.push(current); }

    // First pass: add all entities
    for line in &lines {
        let parts = parse_line(line);
        if let Some(name) = parts.get("SwitchName") {
            let level: u32 = parts.get("Level").and_then(|s| s.parse().ok()).unwrap_or(0);

            ir.add_entity(Entity {
                id: Id(name.clone()),
                kind: EntityKind::Switch { level: Some(level) },
                meta: HashMap::new(),
            });
        }
    }

    // Second pass: add connections according to levels
    for line in &lines {
        let parts = parse_line(line);
        let parent_name = match parts.get("SwitchName") {
            Some(n) => n.clone(),
            None => continue,
        };
        let parent_id = Id(parent_name.clone());
        let parent_level = match ir.entities.get(&parent_id) {
            Some(e) => match e.kind {
                EntityKind::Switch { level: Some(l) } => l,
                _ => continue,
            },
            None => continue,
        };

        // Connect child switches
        if let Some(switches_str) = parts.get("Switches") {
            let children = expand_nodelist(switches_str)?;
            for child_name in children {
                let child_id = Id(child_name.clone());

                let child_level = match ir.entities.get(&child_id) {
                    Some(e) => match e.kind {
                        EntityKind::Switch { level: Some(l) } => l,
                        _ => continue,
                    },
                    None => continue,
                };

                let weight = match (parent_level, child_level) {
                    (2, 1) => 0.5, // L2 -> L1
                    (1, 0) => 1., // L1 -> L0
                    _ => continue, // invalid connection
                };

                ir.add_contains(parent_id.clone(), child_id.clone());
                ir.add_link(parent_id.clone(), child_id, weight);
            }
        }

        // Connect compute nodes only to L0
        if parent_level == 0 {
            if let Some(nodes_str) = parts.get("Nodes") {
                let nodes = expand_nodelist(nodes_str)?
                    .into_iter()
                    .filter(|n| n.starts_with("jpbo")) // only compute nodes
                    .collect::<Vec<_>>();

                for node in nodes {
                    let node_id = Id(node.clone());

                    ir.add_entity(Entity {
                        id: node_id.clone(),
                        kind: EntityKind::Compute,
                        meta: HashMap::new(),
                    });

                    ir.add_contains(parent_id.clone(), node_id.clone());
                    ir.add_link(parent_id.clone(), node_id, 1.);
                }
            }
        }
    }

    Ok(ir)
}
