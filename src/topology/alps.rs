use log::warn;

use crate::{
    ir::{
        Entity, EntityKind, Id, topology_ir::TopologyIR
    },
    parsers::slurm::{NodeListParseError, expand_nodelist, parse_line},
    topology::{TopoSource, resolve_topo_raw},
};
use std::collections::{HashMap, HashSet};

/// One parsed entry from a `scontrol show topology` line, Level=0 only.
#[derive(Debug)]
struct AlpsGroup {
    name: String,           // e.g. "group17"
    nodes: HashSet<String>, // fully-expanded node names
}

/// Parse the raw sinfo text and return only Level=0 switch entries.
/// Level=1 ("global") and any malformed lines are silently skipped.
fn parse_groups(raw: &str) -> Result<Vec<AlpsGroup>, NodeListParseError> {
    let mut groups = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let kv = parse_line(line);

        // Only process leaf-level switch lines.
        let level: u32 = match kv.get("Level").and_then(|v| v.parse().ok()) {
            Some(l) => l,
            None => continue,
        };
        if level != 0 {
            continue;
        }

        let name = match kv.get("SwitchName") {
            Some(n) => n.clone(),
            None => continue,
        };

        let nodes_raw = match kv.get("Nodes") {
            Some(n) => n.clone(),
            // A Level=0 switch with no Nodes key is degenerate — skip.
            None => continue,
        };

        let nodes: HashSet<String> = expand_nodelist(&nodes_raw)?.into_iter().collect();
        groups.push(AlpsGroup { name, nodes });
    }

    Ok(groups)
}

/// For every Level=0 switch entity in the IR, collect the set of compute
/// node IDs that are its direct children via `contains`.
fn compute_children_of_switch<'a>(switch_id: &Id, ir: &'a TopologyIR) -> HashSet<&'a str> {
    ir.contains
        .get(switch_id)
        .map(|children| {
            children
                .iter()
                .filter(|child_id| {
                    ir.entities
                        .get(*child_id)
                        .map(|e| matches!(e.kind, EntityKind::Compute))
                        .unwrap_or(false)
                })
                .map(|child_id| child_id.0.as_str())
                .collect()
        })
        .unwrap_or_default()
}

/// Match each Level=0 switch entity to the sinfo group whose node set has the
/// largest overlap with the switch's compute children.  Returns a map from
/// switch Id → group name.  Switches with zero overlap are not included.
fn match_switches_to_groups<'a>(ir: &TopologyIR, groups: &'a [AlpsGroup]) -> HashMap<Id, &'a str> {
    let mut result = HashMap::new();

    for entity in ir.entities.values() {
        let EntityKind::Switch { level: Some(0) } = &entity.kind else {
            continue;
        };

        let children = compute_children_of_switch(&entity.id, ir);
        if children.is_empty() {
            continue;
        }

        // Pick the group with the highest overlap count.
        let best = groups
            .iter()
            .map(|g| {
                let overlap = children.iter().filter(|n| g.nodes.contains(**n)).count();
                (overlap, g.name.as_str())
            })
            .filter(|(overlap, _)| *overlap > 0)
            .max_by_key(|(overlap, _)| *overlap);

        if let Some((_, group_name)) = best {
            result.insert(entity.id.clone(), group_name);
        }
    }

    result
}

pub fn get_groups_from_topo(
    ir: TopologyIR,
    topo_source: TopoSource,
) -> Result<TopologyIR, NodeListParseError> {
    // ------------------------------------------------------------------ //
    // 1. Obtain and parse the raw topo text.
    // ------------------------------------------------------------------ //
    let raw = resolve_topo_raw(Some(topo_source))?
        .ok_or_else(|| NodeListParseError::new("topo source returned no data"))?;

    let groups = parse_groups(&raw)?;

    // ------------------------------------------------------------------ //
    // 2. Match every Level=0 switch → sinfo group (by node-set overlap).
    // ------------------------------------------------------------------ //
    let switch_to_group = match_switches_to_groups(&ir, &groups);

    for entity in ir.entities.values() {
        let EntityKind::Switch { level: Some(0) } = &entity.kind else {
            continue;
        };
        if !switch_to_group.contains_key(&entity.id) {
            warn!(
                "[get_groups_from_sinfo] WARNING: Level=0 switch '{}' has no matching \
                 sinfo group and will be dropped.",
                entity.id.0
            );
        }
    }

    // ------------------------------------------------------------------ //
    // 3. Find cabinet entities (Switch { level: None }, roots) that are
    //    parents of the matched L0 switches.
    //
    //    Hierarchy: xXXXXcY (level=None, root)
    //                 └─ xXXXXcYrZ (level=Some(0))
    //                      └─ nidXXXXXX (Compute)
    //
    //    Many cabinets → one sinfo group.
    // ------------------------------------------------------------------ //
    let child_to_parent: HashMap<&Id, &Id> = ir
        .contains
        .iter()
        .flat_map(|(parent, children)| children.iter().map(move |child| (child, parent)))
        .collect();

    // cabinet_id → Vec<group_name>  (one vote per L0 switch inside it)
    let mut cabinet_votes: HashMap<Id, Vec<&str>> = HashMap::new();

    for (switch_id, group_name) in &switch_to_group {
        if let Some(cabinet_id) = child_to_parent.get(switch_id) {
            if let Some(entity) = ir.entities.get(*cabinet_id) {
                // Cabinets are Switch { level: None } and are roots.
                if matches!(entity.kind, EntityKind::Switch { level: None }) {
                    cabinet_votes
                        .entry((*cabinet_id).clone())
                        .or_default()
                        .push(group_name);
                }
            }
        }
    }

    let cabinet_to_group: HashMap<Id, &str> = cabinet_votes
        .into_iter()
        .map(|(cabinet_id, votes)| {
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for v in &votes {
                *counts.entry(v).or_insert(0) += 1;
            }
            let winner = counts
                .into_iter()
                .max_by_key(|(_, c)| *c)
                .map(|(name, _)| name)
                .unwrap();

            let all_same = votes.iter().all(|v| *v == winner);
            if !all_same {
                warn!(
                    "[get_groups_from_sinfo] WARNING: cabinet '{}' has conflicting group \
                     votes {:?}; using '{}'.",
                    cabinet_id.0, votes, winner
                );
            }
            (cabinet_id, winner)
        })
        .collect();

    for entity in ir.entities.values() {
        if !matches!(entity.kind, EntityKind::Switch { level: None }) {
            continue;
        }
        if !cabinet_to_group.contains_key(&entity.id) {
            warn!(
                "[get_groups_from_sinfo] WARNING: cabinet '{}' has no matched sinfo \
                 group and will be dropped.",
                entity.id.0
            );
        }
    }

    // ------------------------------------------------------------------ //
    // 4. Build the new IR.
    // ------------------------------------------------------------------ //
    let replaced_cabinets: HashSet<&Id> = cabinet_to_group.keys().collect();

    let unmatched_l0: HashSet<Id> = ir
        .entities
        .values()
        .filter(|e| {
            matches!(e.kind, EntityKind::Switch { level: Some(0) })
                && !switch_to_group.contains_key(&e.id)
        })
        .map(|e| e.id.clone())
        .collect();

    let rewrite_id = |id: &Id| -> Option<Id> {
        if let Some(group_name) = cabinet_to_group.get(id) {
            return Some(Id(group_name.to_string()));
        }
        if unmatched_l0.contains(id) {
            return None;
        }
        Some(id.clone())
    };

    let mut new_ir = TopologyIR::default();

    // -- 4a. Copy all entities that are not being replaced/dropped. ------
    for entity in ir.entities.values() {
        if replaced_cabinets.contains(&entity.id) {
            continue;
        }
        if unmatched_l0.contains(&entity.id) {
            continue;
        }
        new_ir.add_entity(entity.clone());
    }

    // -- 4a'. Update cell/cabinet metadata on matched Level=0 switches. --
    for (switch_id, group_name) in &switch_to_group {
        if let Some(entity) = new_ir.entities.get_mut(switch_id) {
            let old_cell = entity.meta.remove("cell").unwrap_or_default();
            entity.meta.insert("cabinet".to_string(), old_cell);
            entity
                .meta
                .insert("cell".to_string(), group_name.to_string());
        }
    }

    // -- 4b. Create (or merge into) Group entities. ----------------------
    let mut group_xnames: HashMap<&str, Vec<String>> = HashMap::new();
    for (cabinet_id, group_name) in &cabinet_to_group {
        group_xnames
            .entry(group_name)
            .or_default()
            .push(cabinet_id.0.clone());
    }

    for (group_name, xnames) in &group_xnames {
        let group_id = Id(group_name.to_string());
        let mut meta = HashMap::new();
        meta.insert("cell".to_string(), group_name.to_string());
        let mut sorted_xnames = xnames.clone();
        sorted_xnames.sort();
        meta.insert("cabinet_xname".to_string(), sorted_xnames.join(","));
        new_ir.add_entity(Entity {
            id: group_id,
            kind: EntityKind::Group,
            meta,
        });
    }

    // -- 4c. Rewrite contains edges. -------------------------------------
    for (parent_id, children) in &ir.contains {
        let Some(new_parent) = rewrite_id(parent_id) else {
            continue;
        };
        for child_id in children {
            let Some(new_child) = rewrite_id(child_id) else {
                continue;
            };
            new_ir.add_contains(new_parent.clone(), new_child.clone());
        }
    }

    // -- 4d. Rewrite links. ----------------------------------------------
    for link in &ir.links {
        let from = rewrite_id(&link.from);
        let to = rewrite_id(&link.to);
        if let (Some(f), Some(t)) = (from, to) {
            new_ir.add_link(f, t, link.weight);
        }
    }

    Ok(new_ir)
}
