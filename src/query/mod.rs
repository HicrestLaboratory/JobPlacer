mod constraint;
mod executor;
mod reference;
mod error;

pub use constraint::{Constraint, DistanceGroup};
pub use reference::{ReferencePoint, NodePredicate};
pub use executor::TopologyQuery;
pub use error::QueryError;