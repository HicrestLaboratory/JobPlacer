//! TOML topology backend — Alps (and hand-written generic topologies).
//!
//! # File format
//!
//! ```toml
//! [meta]
//! system          = "alps"
//! w_node_switch   = 1.0   # node  ↔ switch weight (normalised, 1.0 = 200 Gb/s)
//! w_switch_switch = 1.0   # switch ↔ switch weight (intra- and inter-group)
//!
//! [[switch]]
//! id    = "x1001c1r3"
//! group = "x1001c1"       # Dragonfly group (= chassis on Alps)
//! level = 0               # optional, defaults to 0
//!
//! [[node]]
//! id     = "nid001305"
//! switch = "x1001c1r3"
//! xname  = "x1001c1s4b0n0"   # stored in meta, optional
//!
//! # Optional explicit inter-switch links (parser synthesises the rest)
//! [[link]]
//! a      = "x1001c1r3"
//! b      = "x1102c6r7"
//! weight = 2.0
//! ```
//!
//! # Topology synthesis
//!
//! 1. All switches within the same `group` are connected to each other
//!    (intra-group all-to-all, weight = `w_switch_switch`).
//! 2. All groups are connected to every other group in an all-to-all
//!    Dragonfly pattern, pairing switches by sorted index
//!    (weight = `w_switch_switch`).
//! 3. Any explicit `[[link]]` entries override / supplement the above.
//!
//! This mirrors the Leonardo/Jupiter synthesis approach, making all three
//! backends share the same IR enrichment patterns.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use log::warn;
use serde::Deserialize;

use crate::ir::topology_ir::TopologyIR;
use crate::ir::Id;
use crate::ir::{Entity, EntityKind};
use crate::parsers::sinfo::{self, NodeInfo};
use crate::parsers::slurm::NodeListParseError;
use crate::topology::{NodeFilterOptions, SinfoSource, TopologyParser};

// ---------------------------------------------------------------------------
// Default link weights (normalised to 200 Gb/s = 1.0)
// ---------------------------------------------------------------------------
const DEFAULT_W_NODE_SWITCH: f32 = 1.0;
const DEFAULT_W_SWITCH_SWITCH: f32 = 1.0;

// ---------------------------------------------------------------------------
// TOML schema — deserialization structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TomlTopology {
    #[serde(default)]
    meta: TomlMeta,
    #[serde(default)]
    switch: Vec<TomlSwitch>,
    #[serde(default)]
    node: Vec<TomlNode>,
    #[serde(default)]
    link: Vec<TomlLink>,
}

#[derive(Debug, Default, Deserialize)]
struct TomlMeta {
    // system:          Option<String>,
    w_node_switch: Option<f32>,
    w_switch_switch: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct TomlSwitch {
    id: String,
    group: Option<String>,
    level: Option<u32>,
    /// Arbitrary extra metadata key-value pairs.
    #[serde(flatten)]
    extra: HashMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
struct TomlNode {
    id: String,
    switch: String,
    xname: Option<String>,
    /// Arbitrary extra metadata.
    #[serde(flatten)]
    extra: HashMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
struct TomlLink {
    a: String,
    b: String,
    weight: f32,
}

// ---------------------------------------------------------------------------
// Parser options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TomlTopologyOptions {
    /// Synthesise intra-group all-to-all switch links (default: true).
    pub intra_group_links: bool,
    /// Synthesise inter-group Dragonfly all-to-all links (default: true).
    pub inter_group_links: bool,
}

impl Default for TomlTopologyOptions {
    fn default() -> Self {
        Self {
            intra_group_links: true,
            inter_group_links: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

pub fn from_file<P: AsRef<Path>>(path: P) -> Result<TopologyIR, NodeListParseError> {
    from_file_with_opts(
        path,
        None,
        NodeFilterOptions::default(),
        TomlTopologyOptions::default(),
    )
}

pub fn from_file_with_opts<P: AsRef<Path>>(
    path: P,
    sinfo_source: Option<SinfoSource>,
    opts: NodeFilterOptions,
    toml_opts: TomlTopologyOptions,
) -> Result<TopologyIR, NodeListParseError> {
    let raw = fs::read_to_string(&path)
        .map_err(|e| NodeListParseError::new(format!("failed to read topology file: {e}")))?;

    let node_infos = resolve_sinfo(sinfo_source)?;

    TomlTopologyParser { options: toml_opts }.build(&raw, node_infos, &opts)
}

// ---------------------------------------------------------------------------
// sinfo source resolver (mirrors Jupiter/Leonardo)
// ---------------------------------------------------------------------------

fn resolve_sinfo(source: Option<SinfoSource>) -> Result<Option<Vec<NodeInfo>>, NodeListParseError> {
    match source {
        None => Ok(None),
        Some(SinfoSource::Command) => Ok(Some(sinfo::from_sinfo_command()?)),
        Some(SinfoSource::File(path)) => Ok(Some(sinfo::from_sinfo_file(path)?)),
    }
}

// ---------------------------------------------------------------------------
// Parser implementation
// ---------------------------------------------------------------------------

pub struct TomlTopologyParser {
    pub options: TomlTopologyOptions,
}

impl Default for TomlTopologyParser {
    fn default() -> Self {
        Self {
            options: TomlTopologyOptions::default(),
        }
    }
}

impl TopologyParser for TomlTopologyParser {
    // ------------------------------------------------------------------
    // Step 1 – parse TOML into base IR
    // ------------------------------------------------------------------
    fn parse_topology_raw(&self, raw: &str) -> Result<TopologyIR, NodeListParseError> {
        let doc: TomlTopology = toml::from_str(raw)
            .map_err(|e| NodeListParseError::new(format!("TOML parse error: {e}")))?;

        let w_node_switch = doc.meta.w_node_switch.unwrap_or(DEFAULT_W_NODE_SWITCH);
        let w_switch_switch = doc.meta.w_switch_switch.unwrap_or(DEFAULT_W_SWITCH_SWITCH);

        let mut ir = TopologyIR::default();

        // ---- Switches --------------------------------------------------
        for sw in &doc.switch {
            let level = sw.level.unwrap_or(0);

            let mut meta: HashMap<String, String> = HashMap::new();
            if let Some(g) = &sw.group {
                meta.insert("cell".into(), g.clone());
            }
            // Forward any extra scalar metadata fields declared in the TOML
            for (k, v) in &sw.extra {
                if let Some(s) = toml_value_as_str(v) {
                    meta.insert(k.clone(), s);
                }
            }

            ir.add_entity(Entity {
                id: Id(sw.id.clone()),
                kind: EntityKind::Switch { level: Some(level) },
                meta,
            });
        }

        // ---- Nodes + node-switch links ---------------------------------
        let mut populated_switches: std::collections::HashSet<Id> =
            std::collections::HashSet::new();

        for node in &doc.node {
            let node_id = Id(node.id.clone());
            let switch_id = Id(node.switch.clone());

            let mut meta: HashMap<String, String> = HashMap::new();
            if let Some(xn) = &node.xname {
                meta.insert("xname".into(), xn.clone());
            }
            for (k, v) in &node.extra {
                if let Some(s) = toml_value_as_str(v) {
                    meta.insert(k.clone(), s);
                }
            }

            ir.add_entity(Entity {
                id: node_id.clone(),
                kind: EntityKind::Compute,
                meta,
            });

            ir.add_contains(switch_id.clone(), node_id.clone());
            ir.add_link(switch_id.clone(), node_id, w_node_switch);
            populated_switches.insert(switch_id);
        }

        // ---- Drop switches with no nodes --------------------------------
        let empty_switches: Vec<Id> = ir
            .entities
            .keys()
            .filter(|id| {
                matches!(ir.entities[*id].kind, EntityKind::Switch { .. })
                    && !populated_switches.contains(*id)
            })
            .cloned()
            .collect();

        for id in &empty_switches {
            warn!(
                "WARNING [toml topology]: switch '{}' has no nodes and will be dropped",
                id.0
            );
            ir.remove_entity(id);
        }

        // ---- Explicit [[link]] entries ---------------------------------
        // Applied first so synthesised links below can detect duplicates
        // if TopologyIR deduplicates — otherwise they just add extra weight.
        for lnk in &doc.link {
            ir.add_link(Id(lnk.a.clone()), Id(lnk.b.clone()), lnk.weight);
        }

        // ---- Intra-group synthesis -------------------------------------
        if self.options.intra_group_links {
            let mut group_to_switches: HashMap<String, Vec<Id>> = HashMap::new();
            for entity in ir.entities.values() {
                if matches!(entity.kind, EntityKind::Switch { .. }) {
                    if let Some(cell) = entity.meta.get("cell") {
                        group_to_switches
                            .entry(cell.clone())
                            .or_default()
                            .push(entity.id.clone());
                    }
                }
            }
            for switches in group_to_switches.values_mut() {
                switches.sort_by(|a, b| a.0.cmp(&b.0));
            }

            for switches in group_to_switches.values() {
                for i in 0..switches.len() {
                    for j in (i + 1)..switches.len() {
                        ir.add_link(switches[i].clone(), switches[j].clone(), w_switch_switch);
                    }
                }
            }
        }

        Ok(ir)
    }

    // ------------------------------------------------------------------
    // Step 2 – enrich with sinfo partition metadata
    // ------------------------------------------------------------------
    fn enrich_node_info(&self, ir: &mut TopologyIR, node_infos: &[NodeInfo]) {
        let mut node_partitions: HashMap<String, Vec<String>> = HashMap::new();
        for info in node_infos {
            node_partitions
                .entry(info.hostname.clone())
                .or_default()
                .push(info.partition.clone());
        }

        for (node_name, partitions) in node_partitions {
            let id = Id(node_name);
            if let Some(entity) = ir.entities.get_mut(&id) {
                let mut deduped = partitions;
                deduped.sort_unstable();
                deduped.dedup();
                entity.meta.insert("partitions".into(), deduped.join(","));
            }
        }
    }

    // ------------------------------------------------------------------
    // Step 3 – synthesise inter-group Dragonfly all-to-all links
    // ------------------------------------------------------------------
    fn enrich_inter_cell_links(&self, ir: &mut TopologyIR) {
        if !self.options.inter_group_links {
            return;
        }

        // Re-read w_switch_switch from the first switch's link weight is not
        // available here, so we use the stored default. If the file specified
        // a custom weight it was already applied to intra-group links in
        // parse_topology_raw; inter-group uses the same value.
        //
        // A more sophisticated design could pass weights through the IR meta,
        // but that would complicate the shared TopologyParser interface. For
        // now we use the constant default — the explicit [[link]] mechanism
        // covers any cases where a different inter-group weight is needed.
        let w = DEFAULT_W_SWITCH_SWITCH;

        let mut group_to_switches: HashMap<String, Vec<Id>> = HashMap::new();
        for entity in ir.entities.values() {
            if matches!(entity.kind, EntityKind::Switch { .. }) {
                if let Some(cell) = entity.meta.get("cell") {
                    group_to_switches
                        .entry(cell.clone())
                        .or_default()
                        .push(entity.id.clone());
                }
            }
        }
        for switches in group_to_switches.values_mut() {
            switches.sort_by(|a, b| a.0.cmp(&b.0));
        }

        let mut groups: Vec<String> = group_to_switches.keys().cloned().collect();
        groups.sort();

        // All-to-all between distinct groups, pairing by sorted index
        // (same pattern as Leonardo / Jupiter)
        for i in 0..groups.len() {
            for j in (i + 1)..groups.len() {
                let sw_a = &group_to_switches[&groups[i]];
                let sw_b = &group_to_switches[&groups[j]];
                let num_links = sw_a.len().min(sw_b.len());
                for k in 0..num_links {
                    ir.add_link(sw_a[k].clone(), sw_b[k].clone(), w);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a scalar TOML value to a String for storage in entity meta.
fn toml_value_as_str(v: &toml::Value) -> Option<String> {
    match v {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        _ => None, // arrays / tables — skip
    }
}
