use crate::ir::entity::{Entity, EntityKind};
use crate::ir::id::Id;
use crate::ir::topology_ir::TopologyIR;
use serde::Serialize;
use std::fs::File;
use std::io::Write;

/// Export TopologyIR to YAML, respecting link weights
pub fn save_ir_as_yaml<P: AsRef<std::path::Path>>(ir: &TopologyIR, path: P) -> std::io::Result<()> {
    #[derive(Serialize)]
    struct YamlEntity {
        id: String,
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        level: Option<u32>,
    }

    #[derive(Serialize)]
    struct YamlContainsChild {
        id: String,
        weight: u32,
    }

    #[derive(Serialize)]
    struct YamlContains {
        parent: String,
        children: Vec<YamlContainsChild>,
    }

    #[derive(Serialize)]
    struct YamlIR {
        entities: Vec<YamlEntity>,
        contains: Vec<YamlContains>,
    }

    // Export entities
    let entities_yaml: Vec<YamlEntity> = ir.entities.values().map(|e| {
        let kind_str = match &e.kind {
            EntityKind::Compute => "compute",
            EntityKind::Switch { .. } => "switch",
            EntityKind::Group => "group",
        }.to_string();

        let level = match &e.kind {
            EntityKind::Switch { level } => *level,
            _ => None,
        };

        YamlEntity {
            id: e.id.0.clone(),
            kind: kind_str,
            level,
        }
    }).collect();

    // Export contains with proper weights
    let contains_yaml: Vec<YamlContains> = ir.contains.iter().map(|(parent, children)| {
        let children_yaml: Vec<YamlContainsChild> = children.iter().map(|child_id: &Id| {
            // Look up the weight from links
            let weight = ir.links.iter()
                .find(|link| link.from == *parent && link.to == *child_id)
                .map(|link| link.weight)
                .unwrap_or(1); // fallback if missing

            YamlContainsChild {
                id: child_id.0.clone(),
                weight,
            }
        }).collect();

        YamlContains {
            parent: parent.0.clone(),
            children: children_yaml,
        }
    }).collect();

    let yaml_ir = YamlIR {
        entities: entities_yaml,
        contains: contains_yaml,
    };

    let yaml_string = serde_yaml::to_string(&yaml_ir)
        .expect("Failed to serialize TopologyIR to YAML");

    let mut file = File::create(path)?;
    file.write_all(yaml_string.as_bytes())?;

    Ok(())
}
