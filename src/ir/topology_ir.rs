use std::collections::{HashMap, HashSet};
use crate::ir::entity::Entity;
use crate::ir::id::Id;
use crate::ir::link::Link;

/// Intermediate representation of the cluster topology
#[derive(Default, Debug, Clone)]
pub struct TopologyIR {
    pub entities: HashMap<Id, Entity>,
    pub links: Vec<Link>,
    pub contains: HashMap<Id, Vec<Id>>,
}

impl TopologyIR {
    pub fn add_entity(&mut self, entity: Entity) {
        self.entities.insert(entity.id.clone(), entity);
    }

    pub fn add_link(&mut self, from: Id, to: Id, weight: f32) {
        self.links.push(Link { from, to, weight });
    }

    pub fn add_contains(&mut self, parent: Id, child: Id) {
        self.contains.entry(parent).or_default().push(child);
    }

    /// Filter the topology keeping only entities that match the predicate
    pub fn filter<F>(&self, predicate: F) -> TopologyIR 
    where
        F: Fn(&Entity) -> bool,
    {
        let mut filtered = TopologyIR::default();
        
        // Collect IDs of entities that pass the filter
        let valid_ids: HashSet<Id> = self.entities
            .values()
            .filter(|entity| predicate(entity))
            .map(|entity| entity.id.clone())
            .collect();
        
        // Add filtered entities
        for id in &valid_ids {
            if let Some(entity) = self.entities.get(id) {
                filtered.add_entity(entity.clone());
            }
        }
        
        // Add links where both endpoints exist in filtered entities
        for link in &self.links {
            if valid_ids.contains(&link.from) && valid_ids.contains(&link.to) {
                filtered.add_link(link.from.clone(), link.to.clone(), link.weight);
            }
        }
        
        // Add containment relationships where both parent and child exist
        for (parent, children) in &self.contains {
            if valid_ids.contains(parent) {
                for child in children {
                    if valid_ids.contains(child) {
                        filtered.add_contains(parent.clone(), child.clone());
                    }
                }
            }
        }
        
        filtered
    }
    
    /// Filter keeping only specified IDs
    pub fn filter_by_ids(&self, ids: &[Id]) -> TopologyIR {
        let id_set: HashSet<Id> = ids.iter().cloned().collect();
        self.filter(|entity| id_set.contains(&entity.id))
    }
    
    /// Filter removing specified IDs
    pub fn filter_remove_ids(&self, ids: &[Id]) -> TopologyIR {
        let id_set: HashSet<Id> = ids.iter().cloned().collect();
        self.filter(|entity| !id_set.contains(&entity.id))
    }
    
    /// Filter keeping entities matching a set of conditions
    /// 
    /// # Examples
    /// 
    /// ```ignore
    /// // Keep only compute nodes
    /// let filtered = ir.filter(|e| e.node_type == "compute");
    /// 
    /// // Keep nodes with high capacity
    /// let filtered = ir.filter(|e| e.capacity > 100);
    /// 
    /// // Complex filter
    /// let filtered = ir.filter(|e| {
    ///     e.node_type == "compute" && e.zone == "us-west" && e.capacity > 50
    /// });
    /// ```
    pub fn filter_chain(&self) -> FilterChain {
        FilterChain::new(self)
    }

    /// Filter compute nodes but keep their complete topology path
    /// This keeps the target nodes plus all their ancestors (switches, routers, etc.)
    pub fn filter_with_topology(&self, target_ids: &[Id]) -> TopologyIR {
        let mut nodes_to_keep: HashSet<Id> = target_ids.iter().cloned().collect();
        
        // Find all parents recursively
        let mut changed = true;
        while changed {
            changed = false;
            let current_nodes: Vec<Id> = nodes_to_keep.iter().cloned().collect();
            
            for (parent, children) in &self.contains {
                for child in children {
                    if current_nodes.contains(child) && !nodes_to_keep.contains(parent) {
                        nodes_to_keep.insert(parent.clone());
                        changed = true;
                    }
                }
            }
        }
        
        let keep_vec: Vec<Id> = nodes_to_keep.into_iter().collect();
        self.filter_by_ids(&keep_vec)
    }
}

/// Builder pattern for chaining multiple filters
pub struct FilterChain<'a> {
    ir: &'a TopologyIR,
    filters: Vec<Box<dyn Fn(&Entity) -> bool + 'a>>,
}

impl<'a> FilterChain<'a> {
    pub fn new(ir: &'a TopologyIR) -> Self {
        Self {
            ir,
            filters: Vec::new(),
        }
    }
    
    /// Add a filter condition
    pub fn filter<F>(mut self, predicate: F) -> Self 
    where
        F: Fn(&Entity) -> bool + 'a,
    {
        self.filters.push(Box::new(predicate));
        self
    }
    
    /// Keep only specified IDs
    pub fn keep_ids(self, ids: Vec<Id>) -> Self {
        let id_set: HashSet<Id> = ids.into_iter().collect();
        self.filter(move |e| id_set.contains(&e.id))
    }
    
    /// Remove specified IDs
    pub fn remove_ids(self, ids: Vec<Id>) -> Self {
        let id_set: HashSet<Id> = ids.into_iter().collect();
        self.filter(move |e| !id_set.contains(&e.id))
    }
    
    /// Build the filtered topology
    pub fn build(self) -> TopologyIR {
        self.ir.filter(|entity| {
            self.filters.iter().all(|f| f(entity))
        })
    }
}