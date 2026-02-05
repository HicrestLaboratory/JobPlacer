use std::{collections::HashMap, process::Command};
use crate::ir::{
    entity::{Entity, EntityKind},
    id::Id,
    topology_ir::TopologyIR,
};

/// Parse Leonardo topology by executing `scontrol show topology` command
pub fn from_scontrol() -> TopologyIR {
    let output = Command::new("scontrol")
        .arg("show")
        .arg("topology")
        .output()
        .expect("Failed to execute scontrol show topology");
    
    if !output.status.success() {
        panic!("scontrol command failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    
    let stdout = String::from_utf8(output.stdout)
        .expect("Invalid UTF-8 in scontrol output");
    
    parse_topology(&stdout)
}

/// Parse topology from scontrol output string
fn parse_topology(output: &str) -> TopologyIR {
    let mut ir = TopologyIR::default();
    
    for line in output.lines() {
        let line = line.trim();
        
        if line.is_empty() {
            continue;
        }
        
        // Parse the line into key-value pairs
        let parts = parse_line(line);
        
        let switch_name = match parts.get("SwitchName") {
            Some(name) => name.to_string(),
            None => continue, // Skip lines without SwitchName
        };
        
        let level: u32 = parts.get("Level")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        
        let link_speed: u32 = parts.get("LinkSpeed")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        
        // Add the switch entity
        let mut meta = HashMap::new();
        meta.insert("link_speed".to_string(), link_speed.to_string());
        
        ir.add_entity(Entity {
            id: Id(switch_name.clone()),
            kind: EntityKind::Switch { level: Some(level) },
            meta,
        });
        
        // Parse and add compute nodes
        if let Some(nodes_str) = parts.get("Nodes") {
            let node_ids = expand_range(nodes_str);
            for node_id in node_ids {
                // Add compute node entity
                ir.add_entity(Entity {
                    id: Id(node_id.clone()),
                    kind: EntityKind::Compute,
                    meta: HashMap::new(),
                });
                
                // Add containment and link
                ir.add_contains(Id(switch_name.clone()), Id(node_id.clone()));
                ir.add_link(Id(switch_name.clone()), Id(node_id), link_speed);
            }
        }
        
        // Parse and add child switches
        if let Some(switches_str) = parts.get("Switches") {
            let switch_ids = expand_range(switches_str);
            for switch_id in switch_ids {
                // Add containment and link to child switch
                ir.add_contains(Id(switch_name.clone()), Id(switch_id.clone()));
                ir.add_link(Id(switch_name.clone()), Id(switch_id), link_speed);
            }
        }
    }
    
    ir
}

/// Parse a SLURM topology line into key-value pairs
/// Example: "SwitchName=sw1 Level=1 Nodes=cn[01-10]"
fn parse_line(line: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    
    for part in line.split_whitespace() {
        if let Some(eq_pos) = part.find('=') {
            let key = part[..eq_pos].to_string();
            let value = part[eq_pos + 1..].to_string();
            map.insert(key, value);
        }
    }
    
    map
}

/// Expand SLURM range notation into individual IDs
/// Examples:
/// - "cn[01-10]" => ["cn01", "cn02", ..., "cn10"]
/// - "cn[1,5,10]" => ["cn1", "cn5", "cn10"]
/// - "cn[01-05,10-12]" => ["cn01", ..., "cn05", "cn10", "cn11", "cn12"]
/// - "sw1,sw2,sw3" => ["sw1", "sw2", "sw3"]
fn expand_range(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    
    // Handle comma-separated list of ranges/items
    for item in s.split(',') {
        if let Some(bracket_start) = item.find('[') {
            let bracket_end = item.rfind(']').expect("Unmatched bracket");
            let prefix = &item[..bracket_start];
            let range_expr = &item[bracket_start + 1..bracket_end];
            
            // Process the range expression inside brackets
            for range_part in range_expr.split(',') {
                if let Some(dash_pos) = range_part.find('-') {
                    // Range like "01-10"
                    let start_str = &range_part[..dash_pos];
                    let end_str = &range_part[dash_pos + 1..];
                    
                    let start: usize = start_str.parse().expect("Invalid range start");
                    let end: usize = end_str.parse().expect("Invalid range end");
                    let width = start_str.len();
                    
                    for i in start..=end {
                        result.push(format!("{}{:0width$}", prefix, i, width = width));
                    }
                } else {
                    // Single value like "5"
                    result.push(format!("{}{}", prefix, range_part));
                }
            }
        } else {
            // No brackets, just a plain name
            result.push(item.to_string());
        }
    }
    
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_expand_range_simple() {
        assert_eq!(
            expand_range("cn[01-03]"),
            vec!["cn01", "cn02", "cn03"]
        );
    }
    
    #[test]
    fn test_expand_range_multiple() {
        assert_eq!(
            expand_range("cn[01-02,05-06]"),
            vec!["cn01", "cn02", "cn05", "cn06"]
        );
    }
    
    #[test]
    fn test_expand_range_comma_list() {
        assert_eq!(
            expand_range("sw1,sw2,sw3"),
            vec!["sw1", "sw2", "sw3"]
        );
    }
    
    #[test]
    fn test_expand_range_mixed() {
        assert_eq!(
            expand_range("isw[100,102,104]"),
            vec!["isw100", "isw102", "isw104"]
        );
    }
    
    #[test]
    fn test_expand_range_complex() {
        assert_eq!(
            expand_range("lrdn[0001-0008,0013-0014]"),
            vec!["lrdn0001", "lrdn0002", "lrdn0003", "lrdn0004",
                 "lrdn0005", "lrdn0006", "lrdn0007", "lrdn0008",
                 "lrdn0013", "lrdn0014"]
        );
    }
    
    #[test]
    fn test_parse_line() {
        let line = "SwitchName=sw1 Level=1 LinkSpeed=1 Nodes=cn[01-10]";
        let parts = parse_line(line);
        
        assert_eq!(parts.get("SwitchName"), Some(&"sw1".to_string()));
        assert_eq!(parts.get("Level"), Some(&"1".to_string()));
        assert_eq!(parts.get("Nodes"), Some(&"cn[01-10]".to_string()));
    }
    
    #[test]
    fn test_parse_topology_sample() {
        let sample = r#"SwitchName=isw-L2-CELLG1 Level=1 LinkSpeed=1 Nodes=lrdn[0001-0180] Switches=isw1[0100,0102,0104]
SwitchName=isw10100 Level=0 LinkSpeed=1 Nodes=lrdn[0001-0008,0013-0014]"#;
        
        let topology = parse_topology(sample);
        
        // Verify switches were created
        // Note: You'll need to adapt this to your actual TopologyIR API
        // This is just an example of what you might test
    }
}