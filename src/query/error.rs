use crate::ir::id::Id;
use std::fmt;

#[derive(Debug, Clone)]
pub enum QueryError {
    /// Not enough nodes available to satisfy the constraint
    InsufficientNodes {
        required: usize,
        available: usize,
    },
    
    /// No valid reference node could be found
    NoValidReference,
    
    /// Reference node is not a compute node
    InvalidAnchor(Id),
    
    /// Constraints conflict with each other
    ConstraintConflict(String),
    
    /// Distance calculation failed
    DistanceCalculationFailed {
        from: Id,
        to: Id,
    },
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::InsufficientNodes { required, available } => {
                write!(
                    f,
                    "Insufficient nodes: required {}, only {} available",
                    required, available
                )
            }
            QueryError::NoValidReference => {
                write!(f, "No valid reference node found")
            }
            QueryError::InvalidAnchor(id) => {
                write!(f, "Invalid anchor node: {}", id.0)
            }
            QueryError::ConstraintConflict(msg) => {
                write!(f, "Constraint conflict: {}", msg)
            }
            QueryError::DistanceCalculationFailed { from, to } => {
                write!(f, "Failed to calculate distance from {} to {}", from.0, to.0)
            }
        }
    }
}

impl std::error::Error for QueryError {}