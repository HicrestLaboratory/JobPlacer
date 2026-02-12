use pyo3::prelude::*;
use pyo3::exceptions::PyRuntimeError;
use crate::ir::id::Id;
use crate::ir::topology_ir::TopologyIR;
use crate::parsers::leonardo;
use crate::query::{TopologyQuery, Constraint, ReferencePoint, DistanceGroup, DistanceGroupWithParent};

/// Format node IDs as comma-separated list
fn format_nodelist_simple(nodes: &[Id]) -> String {
    nodes.iter()
        .map(|id| id.0.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// Format with compression (SLURM range notation)
fn format_nodelist_compressed(nodes: &[Id]) -> String {
    if nodes.is_empty() {
        return String::new();
    }
    
    let node_strings: Vec<&str> = nodes.iter()
        .map(|id| id.0.as_str())
        .collect();
    
    // Try to compress, fallback to simple format
    try_compress_nodes(&node_strings)
        .unwrap_or_else(|| format_nodelist_simple(nodes))
}

/// Try to compress node names into SLURM range notation
fn try_compress_nodes(nodes: &[&str]) -> Option<String> {
    if nodes.is_empty() {
        return None;
    }
    
    // Extract prefix and numeric suffix for each node
    let mut parsed: Vec<(String, usize)> = Vec::new();
    
    for node in nodes {
        if let Some((prefix, num)) = split_prefix_number(node) {
            parsed.push((prefix, num));
        } else {
            return None;
        }
    }
    
    // Check if all nodes share the same prefix
    let first_prefix = &parsed[0].0;
    if !parsed.iter().all(|(prefix, _)| prefix == first_prefix) {
        return None;
    }
    
    // Extract just the numbers and sort
    let mut numbers: Vec<usize> = parsed.iter().map(|(_, num)| *num).collect();
    numbers.sort_unstable();
    
    // Build ranges
    let ranges = build_ranges(&numbers);
    
    Some(format!("{}[{}]", first_prefix, ranges))
}

/// Split a node name into prefix and numeric suffix
fn split_prefix_number(s: &str) -> Option<(String, usize)> {
    let mut num_start = s.len();
    
    for (i, ch) in s.chars().rev().enumerate() {
        if !ch.is_ascii_digit() {
            num_start = s.len() - i;
            break;
        }
    }
    
    if num_start == s.len() {
        return None;
    }
    
    let prefix = &s[..num_start];
    let number_str = &s[num_start..];
    
    number_str.parse::<usize>().ok().map(|num| (prefix.to_string(), num))
}

/// Build range notation from sorted numbers
fn build_ranges(numbers: &[usize]) -> String {
    if numbers.is_empty() {
        return String::new();
    }
    
    let mut ranges = Vec::new();
    let mut range_start = numbers[0];
    let mut range_end = numbers[0];
    
    for &num in &numbers[1..] {
        if num == range_end + 1 {
            range_end = num;
        } else {
            ranges.push(format_range(range_start, range_end));
            range_start = num;
            range_end = num;
        }
    }
    
    ranges.push(format_range(range_start, range_end));
    ranges.join(",")
}

/// Format a single range
fn format_range(start: usize, end: usize) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{}-{}", start, end)
    }
}

/// Python wrapper for the query system
#[pyclass]
struct TopologyQueryBuilder {
    ir: TopologyIR,
}

#[pymethods]
impl TopologyQueryBuilder {
    #[new]
    fn new(leonardo_file: String) -> PyResult<Self> {
        let ir = leonardo::from_file(&leonardo_file);
        Ok(TopologyQueryBuilder { ir })
    }
    
    /// Get nodelist for nodes at specific distances
    /// 
    /// Args:
    ///     anchor: Anchor node ID (string)
    ///     distances: List of (count, distance) tuples
    ///     
    /// Returns:
    ///     Comma-separated nodelist string
    fn get_nodelist_distances(
        &self,
        anchor: String,
        distances: Vec<(usize, f32)>
    ) -> PyResult<String> {
        let anchor_id = Id::from(anchor.as_str());
        
        let groups: Vec<DistanceGroup> = distances
            .into_iter()
            .map(|(count, distance)| DistanceGroup { count, distance })
            .collect();
        
        let query = TopologyQuery::new()
            .with_constraint(Constraint::DistanceGroup {
                reference: ReferencePoint::First,
                groups,
            });
        
        let selected = query.execute_from(&self.ir, anchor_id)
            .map_err(|e| PyRuntimeError::new_err(format!("{}", e)))?;
        
        Ok(format_nodelist_compressed(&selected))
    }
    
    /// Get nodelist for nodes at specific distances with shared parent constraint
    /// 
    /// Args:
    ///     anchor: Anchor node ID (string)
    ///     distances: List of (count, distance, parent_level) tuples
    ///     
    /// Returns:
    ///     Comma-separated nodelist string
    fn get_nodelist_distances_shared_parent(
        &self,
        anchor: String,
        distances: Vec<(usize, f32, usize)>
    ) -> PyResult<String> {
        let anchor_id = Id::from(anchor.as_str());
        
        let groups: Vec<DistanceGroupWithParent> = distances
            .into_iter()
            .map(|(count, distance, parent_level)| {
                DistanceGroupWithParent { count, distance, parent_level }
            })
            .collect();
        
        let query = TopologyQuery::new()
            .with_constraint(Constraint::DistanceGroupWithSharedParent {
                reference: ReferencePoint::First,
                groups,
            });
        
        let selected = query.execute_from(&self.ir, anchor_id)
            .map_err(|e| PyRuntimeError::new_err(format!("{}", e)))?;
        
        Ok(format_nodelist_compressed(&selected))
    }
    
    /// Get nodelist emulating Nanjing topology
    /// (2 nodes at distance 2, 2 nodes at distance 4)
    fn get_nodelist_emulating_nanjing(&self, anchor: String, total_nodes: usize) -> PyResult<String> {
        let anchor_id = Id::from(anchor.as_str());
        
        // Nanjing pattern: groups of 2 nodes at different distances
        let mut groups = vec![];
        let mut remaining = total_nodes - 1; // -1 for anchor
        
        // Alternate between distance 2 and 4
        let mut distance = 2.0;
        while remaining > 0 {
            let count = remaining.min(2);
            groups.push(DistanceGroup { count, distance });
            remaining -= count;
            distance = if distance == 2.0 { 4.0 } else { 2.0 };
        }
        
        let query = TopologyQuery::new()
            .with_constraint(Constraint::DistanceGroup {
                reference: ReferencePoint::First,
                groups,
            });
        
        let selected = query.execute_from(&self.ir, anchor_id)
            .map_err(|e| PyRuntimeError::new_err(format!("{}", e)))?;
        
        Ok(format_nodelist_compressed(&selected))
    }
    
    /// Get nodelist with different distances pattern
    fn get_nodelist_different_distances(&self, anchor: String, total_nodes: usize) -> PyResult<String> {
        let anchor_id = Id::from(anchor.as_str());
        
        let mut groups = vec![];
        let mut remaining = total_nodes - 1; // -1 for anchor
        
        // Pattern: 2@dist2, 2@dist4, 2@dist5, then repeat
        let pattern = vec![2.0, 4.0, 5.0];
        let mut pattern_idx = 0;
        
        while remaining > 0 {
            let count = remaining.min(2);
            groups.push(DistanceGroup { 
                count, 
                distance: pattern[pattern_idx % pattern.len()]
            });
            remaining -= count;
            pattern_idx += 1;
        }
        
        let query = TopologyQuery::new()
            .with_constraint(Constraint::DistanceGroup {
                reference: ReferencePoint::First,
                groups,
            });
        
        let selected = query.execute_from(&self.ir, anchor_id)
            .map_err(|e| PyRuntimeError::new_err(format!("{}", e)))?;
        
        Ok(format_nodelist_compressed(&selected))
    }
    
    /// Get list of available compute nodes
    fn get_compute_nodes(&self) -> Vec<String> {
        use crate::ir::entity::EntityKind;
        
        self.ir.entities
            .iter()
            .filter(|(_, e)| matches!(e.kind, EntityKind::Compute))
            .map(|(id, _)| id.0.clone())
            .collect()
    }
    
    /// Check if a node exists and is a compute node
    fn is_valid_compute_node(&self, node_id: String) -> bool {
        use crate::ir::entity::EntityKind;
        
        let id = Id::from(node_id.as_str());
        self.ir.entities
            .get(&id)
            .map(|e| matches!(e.kind, EntityKind::Compute))
            .unwrap_or(false)
    }
}

/// Python module
#[pymodule]
fn job_placer(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<TopologyQueryBuilder>()?;
    Ok(())
}