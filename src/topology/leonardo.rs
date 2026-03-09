//! Leonardo supercomputer topology backend.
//!
//! # Topology structure
//!
//! Leonardo is a Dragonfly network organised as follows:
//!
//! ```text
//! Cell (CELLG1..CELLG22)
//!  └─ Rack (6 per cell, identified by RR digits in switch name)
//!      └─ Switch pair (3 per rack, identified by pair_index 0..2)
//!          ├─ L1 switch (isw<RR><rr><SS>, SS even)  → compute nodes
//!          └─ L2 switch (isw<RR><rr><SS>, SS odd)   → inter-cell fabric
//! ```
//!
//! Groups (cells) are **not** IR entities. The mapping is preserved via
//! switch metadata:
//!   - `meta["cell"]`       — e.g. `"CELLG1"`
//!   - `meta["rack"]`       — e.g. `"01"` (the RR digits)
//!   - `meta["pair_index"]` — `"0"`, `"1"`, or `"2"` within the rack
//!
//! Inter-cell (dragonfly) links connect the k-th L2 switch of each cell to
//! the k-th L2 switch of every other cell (`INTER_CELL_LINKS_PER_PAIR`
//! controls how many parallel links are modelled).
//!
//! Intra-pair L1↔L2 links are added when `LeonardoOptions::intra_pair_links`
//! is `true` (the default).

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::ir::entity::{Entity, EntityKind};
use crate::ir::id::Id;
use crate::ir::topology_ir::TopologyIR;
use crate::parsers::sinfo::{self, NodeInfo};
use crate::parsers::slurm::{expand_nodelist, parse_line, NodeListParseError};
use crate::topology::{NodeFilterOptions, TopologyParser};

// ---------------------------------------------------------------------------
// Tuneable constants
// ---------------------------------------------------------------------------

/// Number of inter-cell (dragonfly) links to model between each pair of cells.
/// Leonardo has 18 independent 200 Gbps connections between every two cells.
/// Set to 1 to collapse them into a single aggregated link.
pub const INTER_CELL_LINKS_PER_PAIR: usize = 18;

// ---------------------------------------------------------------------------
// Link weights (normalised to 100 Gbps base unit)
// ---------------------------------------------------------------------------
const W_L1_NODE: f32 = 1.0; // 100 Gbps  compute  ↔ L1
const W_L1_L2: f32 = 1.0; // 100 Gbps  L1       ↔ L2  (intra-pair)
const W_INTER_CELL: f32 = 2.0; // 200 Gbps  L2       ↔ L2  (inter-cell)

// ---------------------------------------------------------------------------
// Parser options
// ---------------------------------------------------------------------------

/// Behavioural knobs for [`LeonardoParser`].
#[derive(Debug, Clone)]
pub struct LeonardoOptions {
    /// When `true` (default) an explicit IR link is added between each L1/L2
    /// switch pair.  When `false` the relationship is only reflected in the
    /// shared `meta["rack"]` / `meta["pair_index"]` fields.
    pub intra_pair_links: bool,
}

impl Default for LeonardoOptions {
    fn default() -> Self {
        Self {
            intra_pair_links: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

pub fn from_scontrol() -> Result<TopologyIR, NodeListParseError> {
    from_scontrol_with_opts(
        None,
        NodeFilterOptions::default(),
        LeonardoOptions::default(),
    )
}

pub fn from_file<P: AsRef<Path>>(path: P) -> Result<TopologyIR, NodeListParseError> {
    from_file_with_opts(
        path,
        None,
        NodeFilterOptions::default(),
        LeonardoOptions::default(),
    )
}

pub fn from_scontrol_with_opts(
    sinfo_source: Option<SinfoSource>,
    opts: NodeFilterOptions,
    leonardo_opts: LeonardoOptions,
) -> Result<TopologyIR, NodeListParseError> {
    let topology_raw = run_scontrol_show_topology();
    let node_infos = resolve_sinfo(sinfo_source)?;
    LeonardoParser {
        options: leonardo_opts,
    }
    .build(&topology_raw, node_infos, &opts)
}

pub fn from_file_with_opts<P: AsRef<Path>>(
    path: P,
    sinfo_source: Option<SinfoSource>,
    opts: NodeFilterOptions,
    leonardo_opts: LeonardoOptions,
) -> Result<TopologyIR, NodeListParseError> {
    let topology_raw = fs::read_to_string(path)
        .map_err(|e| NodeListParseError::new(format!("failed to read topology file: {e}")))?;
    let node_infos = resolve_sinfo(sinfo_source)?;
    LeonardoParser {
        options: leonardo_opts,
    }
    .build(&topology_raw, node_infos, &opts)
}

// ---------------------------------------------------------------------------
// sinfo source descriptor
// ---------------------------------------------------------------------------

pub enum SinfoSource {
    Command,
    File(std::path::PathBuf),
}

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

pub struct LeonardoParser {
    pub options: LeonardoOptions,
}

impl Default for LeonardoParser {
    fn default() -> Self {
        Self {
            options: LeonardoOptions::default(),
        }
    }
}

impl TopologyParser for LeonardoParser {
    // ------------------------------------------------------------------
    // Step 1 – parse scontrol output into base IR
    // ------------------------------------------------------------------
    fn parse_topology_raw(&self, raw: &str) -> Result<TopologyIR, NodeListParseError> {
        let mut ir = TopologyIR::default();

        // Merge continuation lines
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        for line in raw.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.starts_with("SwitchName=") {
                if !current.is_empty() {
                    lines.push(current.clone());
                }
                current = t.to_string();
            } else {
                current.push(' ');
                current.push_str(t);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }

        // ---- Pass 1: register all real hardware switches -----------
        // Virtual switches (isw-L2-CELL*, HeadSwitch, isws*) are skipped.
        for line in &lines {
            let parts = parse_line(line);
            let name = match parts.get("SwitchName") {
                Some(n) => n,
                None => continue,
            };
            if is_virtual_switch(name) {
                continue;
            }

            let level = switch_level_from_name(name);
            ir.add_entity(Entity {
                id: Id(name.clone()),
                kind: EntityKind::Switch { level },
                meta: HashMap::new(),
            });
        }

        // ---- Pass 2: attach cell / rack / pair_index metadata ------
        // Cell-group lines list their member switches under Switches=.
        // From the switch name we can derive rack (RR) and pair_index.
        for line in &lines {
            let parts = parse_line(line);
            let group_name = match parts.get("SwitchName") {
                Some(n) => n,
                None => continue,
            };

            // Only process cell groups (isw-L2-CELL*)
            if !is_cell_group(group_name) {
                continue;
            }
            let cell_id = cell_id_from_group_name(group_name);

            if let Some(switches_str) = parts.get("Switches") {
                // Track per-rack switch pairs to assign pair_index.
                // Within a rack the switches are ordered by SS: 00, 02, 04 →
                // pair indices 0, 1, 2.
                let mut rack_switches: HashMap<String, Vec<(u32, Id)>> = HashMap::new();

                for sw_name in expand_nodelist(switches_str)? {
                    let sw_id = Id(sw_name.clone());
                    if let Some(rack) = rack_from_name(&sw_name) {
                        let ss = ss_from_name(&sw_name).unwrap_or(0);
                        rack_switches.entry(rack).or_default().push((ss, sw_id));
                    }
                }

                for (rack, mut entries) in rack_switches {
                    // Sort by SS so pair_index is stable
                    entries.sort_by_key(|(ss, _)| *ss);

                    for (pair_index, (_, sw_id)) in entries.iter().enumerate() {
                        if let Some(entity) = ir.entities.get_mut(sw_id) {
                            entity.meta.insert("cell".into(), cell_id.clone());
                            entity.meta.insert("rack".into(), rack.clone());
                            entity
                                .meta
                                .insert("pair_index".into(), pair_index.to_string());
                        }
                    }
                }
            }
        }

        // ---- Pass 3: compute nodes + L1 containment + intra-pair links
        for line in &lines {
            let parts = parse_line(line);
            let sw_name = match parts.get("SwitchName") {
                Some(n) => n.clone(),
                None => continue,
            };
            if is_virtual_switch(&sw_name) {
                continue;
            }

            let sw_id = Id(sw_name.clone());

            // Only L1 switches connect directly to compute nodes
            let is_l1 = matches!(
                ir.entities.get(&sw_id),
                Some(Entity {
                    kind: EntityKind::Switch { level: Some(0) },
                    ..
                })
            );
            if !is_l1 {
                continue;
            }

            if let Some(nodes_str) = parts.get("Nodes") {
                for node in expand_nodelist(nodes_str)?
                    .into_iter()
                    .filter(|n| n.starts_with("lrdn"))
                {
                    let node_id = Id(node.clone());
                    ir.add_entity(Entity {
                        id: node_id.clone(),
                        kind: EntityKind::Compute,
                        meta: HashMap::new(),
                    });
                    ir.add_contains(sw_id.clone(), node_id.clone());
                    ir.add_link(sw_id.clone(), node_id, W_L1_NODE);
                }
            }

            // Intra-pair link: find the L2 partner (same rack, SS+1)
            if self.options.intra_pair_links {
                if let Some(l2_partner) = l2_partner_of(&sw_name, &ir) {
                    ir.add_link(sw_id.clone(), l2_partner, W_L1_L2);
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
                let mut deduped = partitions.clone();
                deduped.sort_unstable();
                deduped.dedup();
                entity.meta.insert("partitions".into(), deduped.join(","));
            }
        }
    }

    // ------------------------------------------------------------------
    // Step 3 – synthesise inter-cell dragonfly links
    // ------------------------------------------------------------------
    fn enrich_inter_cell_links(&self, ir: &mut TopologyIR) {
        // Collect L2 switches grouped by cell, sorted for stable pairing
        let mut cell_to_l2: HashMap<String, Vec<Id>> = HashMap::new();

        for entity in ir.entities.values() {
            if let EntityKind::Switch { level: Some(1) } = entity.kind {
                if let Some(cell) = entity.meta.get("cell") {
                    cell_to_l2
                        .entry(cell.clone())
                        .or_default()
                        .push(entity.id.clone());
                }
            }
        }

        // Sort each cell's L2 list so the k-th switch is consistent
        for switches in cell_to_l2.values_mut() {
            switches.sort_by(|a, b| a.0.cmp(&b.0));
        }

        let cells: Vec<String> = {
            let mut c: Vec<_> = cell_to_l2.keys().cloned().collect();
            c.sort();
            c
        };

        // All-to-all between distinct cells: pair by sorted index
        for i in 0..cells.len() {
            for j in (i + 1)..cells.len() {
                let sw_a = &cell_to_l2[&cells[i]];
                let sw_b = &cell_to_l2[&cells[j]];

                let num_links = INTER_CELL_LINKS_PER_PAIR.min(sw_a.len()).min(sw_b.len());

                for k in 0..num_links {
                    ir.add_link(sw_a[k].clone(), sw_b[k].clone(), W_INTER_CELL);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Name helpers
// ---------------------------------------------------------------------------

/// Real hardware switches: `isw` followed immediately by a digit.
/// Everything else (HeadSwitch, isw-L2-CELL*, isws*) is virtual.
fn is_virtual_switch(name: &str) -> bool {
    match name.strip_prefix("isw") {
        Some(rest) => !rest.starts_with(|c: char| c.is_ascii_digit()),
        None => true,
    }
}

/// Cell groups are virtual switches whose name contains "CELL".
fn is_cell_group(name: &str) -> bool {
    is_virtual_switch(name) && name.contains("CELL")
}

/// Switch naming: `isw<RR><rr><SS>`
///   RR  = rack-row  (2 digits)
///   rr  = rack-col  (2 digits)  — together RR+rr identify a rack
///   SS  = switch id within rack (2 digits, even=L1, odd=L2)
///
/// Level: even SS → 0 (L1), odd SS → 1 (L2)
fn switch_level_from_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("isw")?;
    if digits.len() < 2 {
        return None;
    }
    let ss: u32 = digits[digits.len() - 2..].parse().ok()?;
    Some(if ss % 2 == 0 { 0 } else { 1 })
}

/// Extract the rack identifier (RR+rr, i.e. all digits except last 2).
fn rack_from_name(name: &str) -> Option<String> {
    let digits = name.strip_prefix("isw")?;
    if digits.len() <= 2 {
        return None;
    }
    Some(digits[..digits.len() - 2].to_string())
}

/// Extract the SS value (last 2 digits).
fn ss_from_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("isw")?;
    if digits.len() < 2 {
        return None;
    }
    digits[digits.len() - 2..].parse().ok()
}

/// Given an L1 switch name, find its L2 partner in the IR.
/// The L2 partner has the same rack prefix and SS+1.
fn l2_partner_of(l1_name: &str, ir: &TopologyIR) -> Option<Id> {
    let digits = l1_name.strip_prefix("isw")?;
    if digits.len() < 2 {
        return None;
    }
    let rack = &digits[..digits.len() - 2];
    let ss: u32 = digits[digits.len() - 2..].parse().ok()?;
    let l2_name = format!("isw{rack}{:02}", ss + 1);
    let l2_id = Id(l2_name);
    ir.entities.contains_key(&l2_id).then_some(l2_id)
}

/// `isw-L2-CELLG21` → `"CELLG21"`
fn cell_id_from_group_name(name: &str) -> String {
    name.rsplit('-').next().unwrap_or(name).to_string()
}

fn run_scontrol_show_topology() -> String {
    let output = Command::new("scontrol")
        .args(["-d", "show", "topology"])
        .output()
        .expect("failed to execute `scontrol show topology`");
    if !output.status.success() {
        panic!(
            "scontrol failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).expect("invalid UTF-8 in scontrol output")
}
