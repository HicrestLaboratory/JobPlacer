use std::{collections::HashMap, fs::File, path::Path};
use serde::Deserialize;

use crate::ir::{
    entity::{Entity, EntityKind},
    id::Id,
    topology_ir::TopologyIR,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawKind {
    Compute,
    Switch,
    Group,
}

#[derive(Debug, Deserialize)]
struct RawEntity {
    id: String,
    kind: RawKind,
    level: Option<u32>,
    meta: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct RawChild {
    id: String,
    weight: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct RawContains {
    parent: String,
    children: Vec<RawChild>,
}

#[derive(Debug, Deserialize)]
struct RawTopology {
    entities: Vec<RawEntity>,
    contains: Vec<RawContains>,
}

/// Expand ranges like cn[01-10] => cn01, cn02, ..., cn10
fn expand_range(s: &str) -> Vec<String> {
    if let Some(l) = s.find('[') {
        let r = s.find(']').unwrap();
        let prefix = &s[..l];
        let body = &s[l + 1..r];

        let mut out = Vec::new();

        for part in body.split(',') {
            if let Some(dash) = part.find('-') {
                let a: usize = part[..dash].parse().unwrap();
                let b: usize = part[dash + 1..].parse().unwrap();
                let width = part[..dash].len();

                for i in a..=b {
                    out.push(format!("{}{:0width$}", prefix, i));
                }
            } else {
                out.push(format!("{}{}", prefix, part));
            }
        }
        out
    } else {
        vec![s.to_string()]
    }
}

pub fn from_file<P: AsRef<Path>>(path: P) -> TopologyIR {
    let file = File::open(&path).expect("Cannot open topology file");
    let raw: RawTopology = serde_yaml::from_reader(file).expect("Failed to parse YAML");

    let mut ir = TopologyIR::default();

    // Add entities
    for e in raw.entities {
        for id in expand_range(&e.id) {
            let kind = match e.kind {
                RawKind::Compute => EntityKind::Compute,
                RawKind::Switch => EntityKind::Switch { level: e.level },
                RawKind::Group => EntityKind::Group,
            };
            ir.add_entity(Entity { id: Id(id), kind, meta: e.meta.clone().unwrap_or_default() });
        }
    }

    // Add contains + implicit links
    for c in raw.contains {
        let parents = expand_range(&c.parent);

        for p in parents {
            for child_obj in &c.children {
                let weight = child_obj.weight.unwrap_or(1);
                for child in expand_range(&child_obj.id) {
                    ir.add_contains(Id(p.clone()), Id(child.clone()));
                    ir.add_link(Id(p.clone()), Id(child), weight);
                }
            }
        }
    }

    ir
}
