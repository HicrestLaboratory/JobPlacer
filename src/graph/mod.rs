use crate::ir::{topology_ir::TopologyIR, Id};
use petgraph::graph::{Graph as PetGraph, NodeIndex};
use petgraph::Undirected;
use std::collections::HashMap;

pub mod display;

/// Build a PetGraph from TopologyIR, returning both the graph and node indices
pub fn graph_from_ir(ir: &TopologyIR) -> (PetGraph<Id, f32, Undirected>, HashMap<Id, NodeIndex>) {
    // Correct constructor for undirected graph
    let mut graph: PetGraph<Id, f32, Undirected> = PetGraph::new_undirected();
    let mut node_indices: HashMap<Id, NodeIndex> = HashMap::new();

    // Add all nodes
    for entity in ir.entities.values() {
        let idx = graph.add_node(entity.id.clone());
        node_indices.insert(entity.id.clone(), idx);
    }

    // Add all links
    for link in &ir.links {
        let from_idx = node_indices[&link.from];
        let to_idx = node_indices[&link.to];
        graph.add_edge(from_idx, to_idx, link.weight);
    }

    (graph, node_indices)
}

pub fn validate(ir: &TopologyIR) -> Result<(), String> {
    for link in &ir.links {
        if !ir.entities.contains_key(&link.from) {
            return Err(format!("Missing entity {:?}", link.from));
        }
        if !ir.entities.contains_key(&link.to) {
            return Err(format!("Missing entity {:?}", link.to));
        }
    }
    Ok(())
}
