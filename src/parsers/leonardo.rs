use std::{collections::HashMap, process::Command, fs, path::Path};
use crate::ir::entity::{Entity, EntityKind};
use crate::ir::id::Id;
use crate::ir::topology_ir::TopologyIR;

/// Parse Leonardo topology by executing `scontrol show topology`
pub fn from_scontrol() -> TopologyIR {
    let output = Command::new("scontrol")
        .arg("show")
        .arg("topology")
        .output()
        .expect("Failed to execute scontrol show topology");

    if !output.status.success() {
        panic!("scontrol command failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8(output.stdout).expect("Invalid UTF-8 in scontrol output");
    parse_topology(&stdout)
}

/// Parse Leonardo topology from a file (for testing)
pub fn from_file<P: AsRef<Path>>(path: P) -> TopologyIR {
    let content = fs::read_to_string(path).expect("Failed to read topology file");
    parse_topology(&content)
}

/// Parse topology string into TopologyIR
fn parse_topology(output: &str) -> TopologyIR {
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
            let children = expand_range(switches_str);
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
                let nodes = expand_range(nodes_str)
                    .into_iter()
                    .filter(|n| n.starts_with("lrdn")) // only compute nodes
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

    ir
}

/// Parse line into key-value map
fn parse_line(line: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in line.split_whitespace() {
        if let Some(pos) = part.find('=') {
            let key = part[..pos].to_string();
            let val = part[pos+1..].to_string();
            map.insert(key, val);
        }
    }
    map
}

/// Expand SLURM-style ranges
fn expand_range(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let items = split_preserving_brackets(s);

    for item in items {
        let item = item.trim();
        if item.is_empty() { continue; }

        if let Some(start_bracket) = item.find('[') {
            if let Some(end_bracket) = item.rfind(']') {
                let prefix = &item[..start_bracket];
                let range_expr = &item[start_bracket+1..end_bracket];

                for part in range_expr.split(',') {
                    if let Some(dash) = part.find('-') {
                        let start = part[..dash].parse::<usize>().unwrap();
                        let end = part[dash+1..].parse::<usize>().unwrap();
                        let width = part[..dash].len();
                        for i in start..=end {
                            result.push(format!("{}{:0width$}", prefix, i, width=width));
                        }
                    } else {
                        result.push(format!("{}{}", prefix, part));
                    }
                }
            }
        } else {
            result.push(item.to_string());
        }
    }

    result
}

/// Split by commas outside brackets
fn split_preserving_brackets(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    for ch in s.chars() {
        match ch {
            '[' => { depth += 1; current.push(ch); }
            ']' => { depth -= 1; current.push(ch); }
            ',' if depth == 0 => {
                if !current.is_empty() { result.push(current.clone()); current.clear(); }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() { result.push(current); }
    result
}
