use crate::ir::topology_ir::TopologyIR;
use crate::ir::id::Id;
use crate::ir::entity::{Entity, EntityKind};
use crate::builder::graph::graph_from_ir;
use super::{Constraint, ReferencePoint, NodePredicate, QueryError};
use std::collections::{HashMap, HashSet};
use petgraph::algo::dijkstra;
use petgraph::graph::NodeIndex;

/// Query builder for selecting compute nodes based on topology constraints
#[derive(Debug, Clone, Default)]
pub struct TopologyQuery {
    constraints: Vec<Constraint>,
}

impl TopologyQuery {
    /// Create a new empty query
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
        }
    }
    
    /// Add a constraint to the query
    pub fn with_constraint(mut self, constraint: Constraint) -> Self {
        self.constraints.push(constraint);
        self
    }
    
    /// Execute query starting from a specific anchor node
    /// 
    /// The anchor node will be the first node in the result and will be used
    /// as the reference point for any `ReferencePoint::First` constraints.
    /// 
    /// # Arguments
    /// * `ir` - The topology IR containing all entities and links
    /// * `anchor` - The compute node ID to start from
    /// 
    /// # Returns
    /// A vector of compute node IDs satisfying all constraints
    pub fn execute_from(
        &self,
        ir: &TopologyIR,
        anchor: Id,
    ) -> Result<Vec<Id>, QueryError> {
        // Filter to only compute nodes
        let compute_ir = ir.filter(|entity| {
            matches!(entity.kind, EntityKind::Compute)
        });
        
        let mut selector = NodeSelector::new(ir, &compute_ir);
        
        // Pre-select the anchor node
        selector.set_anchor(anchor)?;
        
        // Apply all constraints
        for constraint in &self.constraints {
            selector.apply_constraint(constraint)?;
        }
        
        selector.finalize()
    }
    
    /// Execute query without a pre-set anchor
    /// 
    /// The first constraint will determine which node becomes the anchor.
    /// If using `ReferencePoint::First`, an arbitrary compute node will be selected.
    pub fn execute(&self, ir: &TopologyIR) -> Result<Vec<Id>, QueryError> {
        let compute_ir = ir.filter(|entity| {
            matches!(entity.kind, EntityKind::Compute)
        });
        
        let mut selector = NodeSelector::new(ir, &compute_ir);
        
        for constraint in &self.constraints {
            selector.apply_constraint(constraint)?;
        }
        
        selector.finalize()
    }
}

/// Internal state for building node selection
struct NodeSelector<'a> {
    full_ir: &'a TopologyIR,      // Full topology (for distance calculation)
    compute_ir: &'a TopologyIR,   // Only compute nodes (for selection)
    selected: Vec<Id>,             // Nodes selected in order
    available: HashSet<Id>,        // Available compute node IDs
    distance_cache: HashMap<Id, HashMap<Id, f32>>, // from_id -> (to_id -> distance)
}

impl<'a> NodeSelector<'a> {
    fn new(full_ir: &'a TopologyIR, compute_ir: &'a TopologyIR) -> Self {
        let available: HashSet<Id> = compute_ir.entities.keys().cloned().collect();
        
        Self {
            full_ir,
            compute_ir,
            selected: Vec::new(),
            available,
            distance_cache: HashMap::new(),
        }
    }
    
    /// Set the anchor node (first selected node)
    fn set_anchor(&mut self, anchor: Id) -> Result<(), QueryError> {
        if !self.available.contains(&anchor) {
            return Err(QueryError::InvalidAnchor(anchor));
        }
        
        self.available.remove(&anchor);
        self.selected.push(anchor);
        Ok(())
    }
    
    /// Apply a single constraint
    fn apply_constraint(&mut self, constraint: &Constraint) -> Result<(), QueryError> {
        match constraint {
            Constraint::NodesAtDistance { count, distance, reference } => {
                let ref_node = self.resolve_reference(reference)?;
                let candidates = self.find_nodes_at_distance(&ref_node, *distance);
                self.select_n_from(candidates, *count)?;
            }
            
            Constraint::NodesWithinDistance { count, max_distance, reference } => {
                let ref_node = self.resolve_reference(reference)?;
                let candidates = self.find_nodes_within_distance(&ref_node, *max_distance);
                self.select_n_from(candidates, *count)?;
            }
            
            Constraint::DistanceGroup { reference, groups } => {
                let ref_node = self.resolve_reference(reference)?;
                
                for group in groups {
                    let candidates = self.find_nodes_at_distance(&ref_node, group.distance);
                    self.select_n_from(candidates, group.count)?;
                }
            }
            
            Constraint::NodesAtDistanceWithSharedParent { count, distance, reference, parent_level } => {
                let ref_node = self.resolve_reference(reference)?;
                let candidates = self.find_nodes_at_distance_with_shared_parent(
                    &ref_node, 
                    *distance, 
                    *count, 
                    *parent_level
                )?;
                self.select_n_from(candidates, *count)?;
            }
            
            Constraint::DistanceGroupWithSharedParent { reference, groups } => {
                let ref_node = self.resolve_reference(reference)?;
                
                for group in groups {
                    let candidates = self.find_nodes_at_distance_with_shared_parent(
                        &ref_node,
                        group.distance,
                        group.count,
                        group.parent_level,
                    )?;
                    self.select_n_from(candidates, group.count)?;
                }
            }
            
            Constraint::NodeFilter { predicate } => {
                self.filter_available(predicate);
            }
        }
        
        Ok(())
    }
    
    /// Resolve a reference point to an actual node ID
    fn resolve_reference(&mut self, reference: &ReferencePoint) -> Result<Id, QueryError> {
        match reference {
            ReferencePoint::First => {
                // If no anchor set yet, pick any available node
                if self.selected.is_empty() {
                    let first = self.available.iter().next()
                        .cloned()
                        .ok_or(QueryError::NoValidReference)?;
                    self.set_anchor(first.clone())?;
                    Ok(first)
                } else {
                    self.selected.first()
                        .cloned()
                        .ok_or(QueryError::NoValidReference)
                }
            }
            
            ReferencePoint::NodeId(id) => {
                Ok(id.clone())
            }
            
            ReferencePoint::AnyMatching(predicate) => {
                self.available.iter()
                    .find(|id| {
                        if let Some(entity) = self.compute_ir.entities.get(id) {
                            self.matches_predicate(entity, predicate)
                        } else {
                            false
                        }
                    })
                    .cloned()
                    .ok_or(QueryError::NoValidReference)
            }
        }
    }
    
    /// Find all available nodes at exactly the given distance from a reference node
    fn find_nodes_at_distance(&mut self, from: &Id, distance: f32) -> Vec<Id> {
        // Ensure we have distances from this node
        self.ensure_distances_from(from);
        
        self.available
            .iter()
            .filter(|id| {
                if let Some(dist) = self.get_cached_distance(from, id) {
                    (dist - distance).abs() < 0.001 // Float comparison with epsilon
                } else {
                    false
                }
            })
            .cloned()
            .collect()
    }
    
    /// Find all available nodes within max distance from a reference node
    fn find_nodes_within_distance(&mut self, from: &Id, max_distance: f32) -> Vec<Id> {
        // Ensure we have distances from this node
        self.ensure_distances_from(from);
        
        self.available
            .iter()
            .filter(|id| {
                if let Some(dist) = self.get_cached_distance(from, id) {
                    dist <= max_distance
                } else {
                    false
                }
            })
            .cloned()
            .collect()
    }
    
    /// Find nodes at a specific distance that share the same parent at a given level
    fn find_nodes_at_distance_with_shared_parent(
        &mut self,
        from: &Id,
        distance: f32,
        count: usize,
        parent_level: usize,
    ) -> Result<Vec<Id>, QueryError> {
        // First, get all nodes at the target distance
        let candidates_at_distance = self.find_nodes_at_distance(from, distance);
        
        if candidates_at_distance.is_empty() {
            return Err(QueryError::InsufficientNodes {
                required: count,
                available: 0,
            });
        }
        
        // Group candidates by their parent at the specified level
        let mut groups_by_parent: HashMap<Id, Vec<Id>> = HashMap::new();
        
        for candidate in &candidates_at_distance {
            if let Some(parent) = self.get_ancestor(candidate, parent_level) {
                groups_by_parent.entry(parent).or_default().push(candidate.clone());
            }
        }
        
        // Find a group with at least 'count' nodes
        for (_, nodes) in &groups_by_parent {  // ← Changed: borrowed instead of moved
            if nodes.len() >= count {
                return Ok(nodes.clone());  // ← Changed: clone the vector
            }
        }
        
        // No group has enough nodes
        let max_available = groups_by_parent.values()
            .map(|v| v.len())
            .max()
            .unwrap_or(0);
        
        Err(QueryError::InsufficientNodes {
            required: count,
            available: max_available,
        })
    }
    
    /// Get the ancestor of a node at a specific level
    /// Level 1 = direct parent, Level 2 = grandparent, etc.
    fn get_ancestor(&self, node: &Id, level: usize) -> Option<Id> {
        let mut current = node.clone();
        
        for _ in 0..level {
            // Find parent in contains relationship
            let parent = self.full_ir.contains
                .iter()
                .find(|(_, children)| children.contains(&current))
                .map(|(parent, _)| parent.clone())?;
            
            current = parent;
        }
        
        Some(current)
    }
    
    /// Get cached distance (assumes distances have been computed)
    fn get_cached_distance(&self, from: &Id, to: &Id) -> Option<f32> {
        if from == to {
            return Some(0.0);
        }
        
        self.distance_cache
            .get(from)
            .and_then(|distances: &HashMap<Id, f32>| distances.get(to))
            .copied()
    }
    
    /// Ensure all distances from a reference node are computed and cached
    fn ensure_distances_from(&mut self, from: &Id) {
        // Check if we already have distances from this node
        if self.distance_cache.contains_key(from) {
            return;
        }
        
        // Compute all distances from this node
        let distances = self.compute_all_distances_from(from);
        self.distance_cache.insert(from.clone(), distances);
    }
    
    /// Compute distances from a node to all other nodes using Dijkstra
    fn compute_all_distances_from(&self, from: &Id) -> HashMap<Id, f32> {
        let (graph, node_indices) = graph_from_ir(self.full_ir);
        
        let from_idx: NodeIndex = match node_indices.get(from) {
            Some(idx) => *idx,
            None => return HashMap::new(),
        };
        
        // Compute distances to ALL nodes (None means no early stopping)
        // dijkstra returns hashbrown::HashMap, so we don't specify the type
        let distances = dijkstra(
            &graph,
            from_idx,
            None, // Compute to all nodes
            |edge_ref: petgraph::graph::EdgeReference<f32>| *edge_ref.weight()
        );
        
        // Convert from NodeIndex -> f32 to Id -> f32
        let mut result = HashMap::new();
        for (node_idx, distance) in distances {
            // Find the Id for this NodeIndex
            for (id, idx) in &node_indices {
                if *idx == node_idx {
                    result.insert(id.clone(), distance);
                    break;
                }
            }
        }
        
        result
    }
    
    /// Select N nodes from candidates
    fn select_n_from(&mut self, mut candidates: Vec<Id>, n: usize) -> Result<(), QueryError> {
        if candidates.len() < n {
            return Err(QueryError::InsufficientNodes {
                required: n,
                available: candidates.len(),
            });
        }
        
        // Take first N candidates
        candidates.truncate(n);
        
        for id in &candidates {
            self.available.remove(id);
            self.selected.push(id.clone());
        }
        
        Ok(())
    }
    
    /// Filter available nodes by predicate
    fn filter_available(&mut self, predicate: &NodePredicate) {
        let compute_ir = self.compute_ir;
        self.available.retain(|id| {
            if let Some(entity) = compute_ir.entities.get(id) {
                matches_predicate(entity, predicate)
            } else {
                false
            }
        });
    }
    
    /// Check if entity matches predicate
    fn matches_predicate(&self, entity: &Entity, predicate: &NodePredicate) -> bool {
        matches_predicate(entity, predicate)
    }
    
    /// Finalize and return selected nodes
    fn finalize(self) -> Result<Vec<Id>, QueryError> {
        if self.selected.is_empty() {
            Err(QueryError::InsufficientNodes {
                required: 1,
                available: 0,
            })
        } else {
            Ok(self.selected)
        }
    }
}

/// Helper function to check if entity matches predicate
fn matches_predicate(entity: &Entity, predicate: &NodePredicate) -> bool {
    match predicate {
        NodePredicate::NodeKind(kind) => {
            match &entity.kind {
                EntityKind::Compute => kind == "compute" || kind == "Compute",
                EntityKind::Switch { .. } => kind == "switch" || kind == "Switch",
                EntityKind::Group => kind == "group" || kind == "Group",
            }
        }
        
        NodePredicate::HasProperty(key, value) => {
            entity.meta.get(key).map(|v| v == value).unwrap_or(false)
        }
        
        NodePredicate::IdPattern(pattern) => {
            entity.id.0.contains(pattern)
        }
        
        NodePredicate::Custom(_) => {
            // Custom predicates not implemented yet
            false
        }
    }
}