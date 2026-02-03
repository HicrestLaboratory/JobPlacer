use std::collections::HashMap;
use crate::ir::id::Id;

#[derive(Clone, Debug)]
pub enum EntityKind {
    Compute,
    Switch { level: Option<u32> },
    Group,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub id: Id,
    pub kind: EntityKind,
    pub meta: HashMap<String, String>,
}