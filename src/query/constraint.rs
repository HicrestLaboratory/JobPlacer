use crate::ir::id::Id;
use super::reference::{ReferencePoint, NodePredicate};

/// A constraint for selecting compute nodes
#[derive(Debug, Clone)]
pub enum Constraint {
    /// Select N nodes at exactly distance D from a reference point
    /// 
    /// # Example
    /// ```ignore
    /// // Select 4 nodes at distance 2.0 from first node (same L1 switch)
    /// Constraint::NodesAtDistance {
    ///     count: 4,
    ///     distance: 2.0,
    ///     reference: ReferencePoint::First,
    /// }
    /// ```
    NodesAtDistance {
        count: usize,
        distance: f32,
        reference: ReferencePoint,
    },
    
    /// Select N nodes within max distance D from a reference point
    /// 
    /// # Example
    /// ```ignore
    /// // Select 4 nodes within distance 3.0 of a specific node
    /// Constraint::NodesWithinDistance {
    ///     count: 4,
    ///     max_distance: 3.0,
    ///     reference: ReferencePoint::NodeId("cn1".into()),
    /// }
    /// ```
    NodesWithinDistance {
        count: usize,
        max_distance: f32,
        reference: ReferencePoint,
    },
    
    /// Multiple distance requirements from same reference point
    /// 
    /// # Example
    /// ```ignore
    /// // Select 2 nodes at distance 2.0 and 2 nodes at distance 4.0
    /// Constraint::DistanceGroup {
    ///     reference: ReferencePoint::First,
    ///     groups: vec![
    ///         DistanceGroup { count: 2, distance: 2.0 },
    ///         DistanceGroup { count: 2, distance: 4.0 },
    ///     ],
    /// }
    /// ```
    DistanceGroup {
        reference: ReferencePoint,
        groups: Vec<DistanceGroup>,
    },
    
    /// Filter nodes by predicate
    /// 
    /// # Example
    /// ```ignore
    /// // Only consider nodes with specific property
    /// Constraint::NodeFilter {
    ///     predicate: NodePredicate::HasProperty("rack", "rack1"),
    /// }
    /// ```
    NodeFilter {
        predicate: NodePredicate,
    },
}

/// A group of nodes at a specific distance
#[derive(Debug, Clone)]
pub struct DistanceGroup {
    /// Number of nodes to select at this distance
    pub count: usize,
    
    /// Graph distance (sum of edge weights along shortest path)
    pub distance: f32,
}