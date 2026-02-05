use std::collections::HashMap;

use crate::ir::entity::Entity;
use crate::ir::id::Id;

use crate::ir::link::Link;

/// Intermediate representation of the cluster topology
#[derive(Default, Debug)]
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
}
