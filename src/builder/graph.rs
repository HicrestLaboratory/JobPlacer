use std::collections::HashMap;
use crate::ir::{topology_ir::TopologyIR, id::Id};

#[derive(Debug)]
pub struct Graph {
    pub adj: HashMap<Id, Vec<(Id, u32)>>,
}

impl Graph {
    pub fn from_ir(ir: &TopologyIR) -> Self {
        // Explicit type annotation fixes the inference error
        let mut adj: HashMap<Id, Vec<(Id, u32)>> = HashMap::new();

        for link in &ir.links {
            adj.entry(link.from.clone())
                .or_default()
                .push((link.to.clone(), link.weight));
            adj.entry(link.to.clone())
                .or_default()
                .push((link.from.clone(), link.weight));
        }

        Graph { adj }
    }
}
