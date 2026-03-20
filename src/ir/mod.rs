use std::collections::HashMap;

pub mod topology_ir;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(pub String);

impl From<&str> for Id {
    fn from(s: &str) -> Self {
        Id(s.to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug)]
pub struct Link {
    pub from: Id,
    pub to: Id,
    pub weight: f32,
}
