use crate::ir::id::Id;

/// Reference point for distance-based constraints
#[derive(Debug, Clone)]
pub enum ReferencePoint {
    /// First node selected by the query (anchor node)
    First,
    
    /// Specific node by ID
    NodeId(Id),
    
    /// Any node matching criteria (picks first match)
    AnyMatching(NodePredicate),
}

/// Predicate for filtering nodes
#[derive(Debug, Clone)]
pub enum NodePredicate {
    /// Match nodes by type/kind (e.g., "compute", "switch")
    NodeKind(String),
    
    /// Match nodes with specific property value
    HasProperty(String, String),
    
    /// Match nodes by ID pattern
    IdPattern(String),
    
    /// Custom predicate (for extensibility)
    Custom(String),
}