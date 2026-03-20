use super::reference::{NodePredicate, ReferencePoint};

/// A constraint for selecting compute nodes
#[derive(Debug, Clone)]
pub enum Constraint {
    /// Select N nodes at exactly distance D from a reference point
    NodesAtDistance {
        count: usize,
        distance: f32,
        reference: ReferencePoint,
    },

    /// Select N nodes within max distance D from a reference point
    NodesWithinDistance {
        count: usize,
        max_distance: f32,
        reference: ReferencePoint,
    },

    /// Multiple distance requirements from same reference point
    DistanceGroup {
        reference: ReferencePoint,
        groups: Vec<DistanceGroup>,
    },

    /// Select N nodes at distance D that share the same parent (switch)
    ///
    /// # Example
    /// ```ignore
    /// // Select 2 nodes at distance 4 that are under the same L1 switch
    /// Constraint::NodesAtDistanceWithSharedParent {
    ///     count: 2,
    ///     distance: 4,
    ///     reference: ReferencePoint::First,
    ///     parent_level: 1, // 1 = direct parent (L1 switch)
    /// }
    /// ```
    NodesAtDistanceWithSharedParent {
        count: usize,
        distance: f32,
        reference: ReferencePoint,
        parent_level: usize, // 1 = direct parent, 2 = grandparent, etc.
    },

    /// Multiple distance groups with shared parent constraint
    ///
    /// # Example
    /// ```ignore
    /// // 2 nodes at distance 4 under same switch, 2 nodes at distance 2 under same switch
    /// Constraint::DistanceGroupWithSharedParent {
    ///     reference: ReferencePoint::First,
    ///     groups: vec![
    ///         DistanceGroupWithParent { count: 2, distance: 4, parent_level: 1 },
    ///         DistanceGroupWithParent { count: 2, distance: 2, parent_level: 1 },
    ///     ],
    /// }
    /// ```
    DistanceGroupWithSharedParent {
        reference: ReferencePoint,
        groups: Vec<DistanceGroupWithParent>,
    },

    /// Filter nodes by predicate
    NodeFilter { predicate: NodePredicate },
}

/// A group of nodes at a specific distance
#[derive(Debug, Clone)]
pub struct DistanceGroup {
    /// Number of nodes to select at this distance
    pub count: usize,

    /// Graph distance (sum of edge weights along shortest path)
    pub distance: f32,
}

/// A group of nodes at a specific distance with shared parent constraint
#[derive(Debug, Clone)]
pub struct DistanceGroupWithParent {
    /// Number of nodes to select at this distance
    pub count: usize,

    /// Graph distance (sum of edge weights along shortest path)
    pub distance: f32,

    /// Parent level: 1 = direct parent (L1 switch), 2 = grandparent (L2 switch), etc.
    pub parent_level: usize,
}
