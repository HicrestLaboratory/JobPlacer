use crate::ir::id::Id;

#[derive(Clone, Debug)]
pub struct Link {
    pub from: Id,
    pub to: Id,
    pub weight: u32, // hops, abstract cost
}