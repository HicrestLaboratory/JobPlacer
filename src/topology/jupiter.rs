//! Jupiter supercomputer topology backend.
//!
//! # Topology structure
//!
//! Jupiter is a Dragonfly+ network organised as follows:
//!
//! ```text
//! Group  (25 booster + 1 cluster + 1 admin = 27 total)
//!  └─ Rack  (5 per group, identified by RRR digits in switch/node name)
//!      └─ L1 switch  (jpbi-<RRR>-l1-<NN>, 3 per rack)
//!          └─ Compute nodes  (jpbo-<RRR>-<NN>, 16 per L1 → 48 per rack)
//!  └─ L2 switch  (jpbi-<RRR>-l2-<NN>, 1 per group, anchored to first rack)
//!      └─ (governs all L1s in the group via inter-group fabric)
//! ```
//!
//! Groups are **not** IR entities. The mapping is preserved via metadata:
//!   - `meta["cell"]`      — e.g. `"group-01"` (derived from first rack in group)
//!   - `meta["rack"]`      — e.g. `"001"` (the RRR digits)
//!   - `meta["l1_index"]`  — `"01"`, `"02"`, `"03"` within the rack (L1 only)
//!   - `meta["l2_index"]`  — `"01"`, `"02"`, ... within the group (L2 only)
//!
//! Inter-group (dragonfly+) links are synthesised by connecting the k-th L2
//! of each group to the k-th L2 of every other group.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::ir::entity::{Entity, EntityKind};
use crate::ir::id::Id;
use crate::ir::topology_ir::TopologyIR;
use crate::parsers::run_scontrol_show_topology;
use crate::parsers::sinfo::{self, NodeInfo};
use crate::parsers::slurm::{expand_nodelist, parse_line, NodeListParseError};
use crate::topology::{NodeFilterOptions, SinfoSource, TopologyParser};

// ---------------------------------------------------------------------------
// Tuneable constants
// ---------------------------------------------------------------------------

/// Number of inter-group (dragonfly+) links to model between each group pair.
/// Set to 1 to collapse all parallel links into a single aggregated link.
pub const INTER_GROUP_LINKS_PER_PAIR: usize = 1;

// ---------------------------------------------------------------------------
// Link weights (normalised to 200 Gbps base unit, matching Jupiter's HCA)
// ---------------------------------------------------------------------------
const W_L1_NODE:    f32 = 1.0; // 200 Gbps  compute ↔ L1
const W_L1_L2:      f32 = 2.0; // 400 Gbps  L1      ↔ L2  (intra-group)
const W_INTER_GROUP: f32 = 4.0; // 400 Gbps  L2      ↔ L2  (inter-group, higher weight to distinguish)

// ---------------------------------------------------------------------------
// Parser options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct JupiterOptions {
    /// When `true` (default) an explicit IR link is added between each L1
    /// switch and the group's L2 switch.
    pub intra_group_links: bool,
}

impl Default for JupiterOptions {
    fn default() -> Self {
        Self { intra_group_links: true }
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

pub fn from_scontrol() -> Result<TopologyIR, NodeListParseError> {
    from_scontrol_with_opts(None, NodeFilterOptions::default(), JupiterOptions::default())
}

pub fn from_file<P: AsRef<Path>>(path: P) -> Result<TopologyIR, NodeListParseError> {
    from_file_with_opts(path, None, NodeFilterOptions::default(), JupiterOptions::default())
}

pub fn from_scontrol_with_opts(
    sinfo_source: Option<SinfoSource>,
    opts: NodeFilterOptions,
    jupiter_opts: JupiterOptions,
) -> Result<TopologyIR, NodeListParseError> {
    let topology_raw = run_scontrol_show_topology();
    let node_infos   = resolve_sinfo(sinfo_source)?;
    JupiterParser { options: jupiter_opts }.build(&topology_raw, node_infos, &opts)
}

pub fn from_file_with_opts<P: AsRef<Path>>(
    path: P,
    sinfo_source: Option<SinfoSource>,
    opts: NodeFilterOptions,
    jupiter_opts: JupiterOptions,
) -> Result<TopologyIR, NodeListParseError> {
    let topology_raw = fs::read_to_string(path)
        .map_err(|e| NodeListParseError::new(format!("failed to read topology file: {e}")))?;
    let node_infos = resolve_sinfo(sinfo_source)?;
    JupiterParser { options: jupiter_opts }.build(&topology_raw, node_infos, &opts)
}

// ---------------------------------------------------------------------------
// sinfo source descriptor
// ---------------------------------------------------------------------------

fn resolve_sinfo(
    source: Option<SinfoSource>,
) -> Result<Option<Vec<NodeInfo>>, NodeListParseError> {
    match source {
        None                          => Ok(None),
        Some(SinfoSource::Command)    => Ok(Some(sinfo::from_sinfo_command()?)),
        Some(SinfoSource::File(path)) => Ok(Some(sinfo::from_sinfo_file(path)?)),
    }
}

// ---------------------------------------------------------------------------
// Parser implementation
// ---------------------------------------------------------------------------

pub struct JupiterParser {
    pub options: JupiterOptions,
}

impl Default for JupiterParser {
    fn default() -> Self {
        Self { options: JupiterOptions::default() }
    }
}

impl TopologyParser for JupiterParser {
    // ------------------------------------------------------------------
    // Step 1 – parse scontrol output into base IR
    // ------------------------------------------------------------------
    fn parse_topology_raw(&self, raw: &str) -> Result<TopologyIR, NodeListParseError> {
        let mut ir = TopologyIR::default();

        // Merge continuation lines
        let lines = merge_continuation_lines(raw);

        // ---- Pass 1: register all real hardware switches -----------
        for line in &lines {
            let parts = parse_line(line);
            let name = match parts.get("SwitchName") { Some(n) => n, None => continue };
            if is_head_switch(name) { continue; }

            match parse_switch_name(name) {
                Some(SwitchInfo::L1 { rack, index }) => {
                    ir.add_entity(Entity {
                        id:   Id(name.clone()),
                        kind: EntityKind::Switch { level: Some(0) },
                        meta: HashMap::from([
                            ("rack".into(),     rack),
                            ("l1_index".into(), index),
                        ]),
                    });
                }
                Some(SwitchInfo::L2 { anchor_rack, index }) => {
                    // Cell is derived later in pass 2 once we know all racks
                    // in this L2's Switches= list. Store anchor_rack for now.
                    ir.add_entity(Entity {
                        id:   Id(name.clone()),
                        kind: EntityKind::Switch { level: Some(1) },
                        meta: HashMap::from([
                            ("anchor_rack".into(), anchor_rack),
                            ("l2_index".into(),    index),
                        ]),
                    });
                }
                None => {
                    // Unrecognised switch name — skip silently
                }
            }
        }

        // ---- Pass 2: derive group ("cell") metadata ----------------
        // Each L2 line has Switches=jpbi-[RRR-SSS]-l1-[01-03], which tells
        // us exactly which racks belong to this group.
        for line in &lines {
            let parts = parse_line(line);
            let name = match parts.get("SwitchName") { Some(n) => n, None => continue };
            if !is_l2_switch(name) { continue; }

            let switches_str = match parts.get("Switches") { Some(s) => s, None => continue };
            let member_l1s   = expand_nodelist(switches_str)?;

            // Collect all unique rack IDs mentioned in the L1 list
            let mut racks_in_group: Vec<String> = member_l1s
                .iter()
                .filter_map(|sw| parse_switch_name(sw))
                .filter_map(|info| match info {
                    SwitchInfo::L1 { rack, .. } => Some(rack),
                    _ => None,
                })
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            racks_in_group.sort();

            // Group id: "group-<first_rack>" — stable and human-readable
            let group_id = format!("group-{}", racks_in_group[0]);

            // Tag the L2 switch itself
            if let Some(entity) = ir.entities.get_mut(&Id(name.clone())) {
                entity.meta.insert("cell".into(), group_id.clone());
                entity.meta.remove("anchor_rack");
            }

            // Tag every L1 switch in this group
            for l1_name in &member_l1s {
                if let Some(entity) = ir.entities.get_mut(&Id(l1_name.clone())) {
                    entity.meta.insert("cell".into(), group_id.clone());
                }
            }
        }

        // ---- Pass 3: compute nodes + L1 containment ----------------
        for line in &lines {
            let parts = parse_line(line);
            let sw_name = match parts.get("SwitchName") { Some(n) => n.clone(), None => continue };

            if !is_l1_switch(&sw_name) { continue; }
            let sw_id = Id(sw_name.clone());

            if let Some(nodes_str) = parts.get("Nodes") {
                for node_name in expand_nodelist(nodes_str)?
                    .into_iter()
                    .filter(|n| n.starts_with("jpbo-"))
                {
                    let node_id = Id(node_name.clone());

                    // Inherit rack from node name for quick lookup
                    let rack = rack_from_node_name(&node_name)
                        .unwrap_or_else(|| "?".into());

                    ir.add_entity(Entity {
                        id:   node_id.clone(),
                        kind: EntityKind::Compute,
                        meta: HashMap::from([("rack".into(), rack)]),
                    });
                    ir.add_contains(sw_id.clone(), node_id.clone());
                    ir.add_link(sw_id.clone(), node_id, W_L1_NODE);
                }
            }

            // Intra-group link: L1 → L2 of the same group
            if self.options.intra_group_links {
                let group = ir.entities.get(&sw_id)
                    .and_then(|e| e.meta.get("cell"))
                    .cloned();

                if let Some(cell) = group {
                    // Find the L2 switch(es) for this group
                    let l2s: Vec<Id> = ir.entities.values()
                        .filter(|e| {
                            matches!(e.kind, EntityKind::Switch { level: Some(1) })
                                && e.meta.get("cell").map(|s| s.as_str()) == Some(&cell)
                        })
                        .map(|e| e.id.clone())
                        .collect();

                    for l2_id in l2s {
                        ir.add_link(sw_id.clone(), l2_id, W_L1_L2);
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
    // Step 3 – synthesise inter-group dragonfly+ links
    // ------------------------------------------------------------------
    fn enrich_inter_cell_links(&self, ir: &mut TopologyIR) {
        // Collect L2 switches grouped by cell, sorted for stable pairing
        let mut group_to_l2: HashMap<String, Vec<Id>> = HashMap::new();
        for entity in ir.entities.values() {
            if let EntityKind::Switch { level: Some(1) } = entity.kind {
                if let Some(cell) = entity.meta.get("cell") {
                    group_to_l2.entry(cell.clone()).or_default().push(entity.id.clone());
                }
            }
        }
        for switches in group_to_l2.values_mut() {
            switches.sort_by(|a, b| a.0.cmp(&b.0));
        }

        let mut groups: Vec<String> = group_to_l2.keys().cloned().collect();
        groups.sort();

        // All-to-all between distinct groups: pair by sorted index
        for i in 0..groups.len() {
            for j in (i + 1)..groups.len() {
                let sw_a = &group_to_l2[&groups[i]];
                let sw_b = &group_to_l2[&groups[j]];
                let num_links = INTER_GROUP_LINKS_PER_PAIR.min(sw_a.len()).min(sw_b.len());
                for k in 0..num_links {
                    ir.add_link(sw_a[k].clone(), sw_b[k].clone(), W_INTER_GROUP);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Name parsing
// ---------------------------------------------------------------------------

enum SwitchInfo {
    L1 { rack: String, index: String },
    L2 { anchor_rack: String, index: String },
}

/// Parse `jpbi-<RRR>-l1-<NN>` or `jpbi-<RRR>-l2-<NN>`.
fn parse_switch_name(name: &str) -> Option<SwitchInfo> {
    // Expected format: jpbi-RRR-{l1|l2}-NN
    let rest = name.strip_prefix("jpbi-")?;
    let mut parts = rest.splitn(3, '-');

    let rack  = parts.next()?.to_string();         // "001"
    let level = parts.next()?;                     // "l1" or "l2"
    let index = parts.next()?.to_string();         // "01"

    match level {
        "l1" => Some(SwitchInfo::L1 { rack, index }),
        "l2" => Some(SwitchInfo::L2 { anchor_rack: rack, index }),
        _    => None,
    }
}

fn is_head_switch(name: &str) -> bool {
    name == "HeadSwitch"
}

fn is_l1_switch(name: &str) -> bool {
    name.contains("-l1-")
}

fn is_l2_switch(name: &str) -> bool {
    name.contains("-l2-")
}

/// Extract rack from compute node name: `jpbo-<RRR>-<NN>` → `"RRR"`.
fn rack_from_node_name(name: &str) -> Option<String> {
    name.strip_prefix("jpbo-")
        .and_then(|rest| rest.split('-').next())
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Line merging (same pattern as Leonardo)
// ---------------------------------------------------------------------------

fn merge_continuation_lines(raw: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() { continue; }
        if t.starts_with("SwitchName=") {
            if !current.is_empty() { lines.push(current.clone()); }
            current = t.to_string();
        } else {
            current.push(' ');
            current.push_str(t);
        }
    }
    if !current.is_empty() { lines.push(current); }
    lines
}
