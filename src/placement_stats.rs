// placement_stats.rs
//
// Given a map of job-name → node-list (as returned by the placer or loaded
// from a JSON allocation file), compute topology-aware statistics by looking
// each node up in a `TopologyIR`.
//
// Usage (within the same binary):
//
//   let stats = PlacementStats::compute(&ir, &allocation);
//   let json   = serde_json::to_string_pretty(&stats)?;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::ir::topology_ir::TopologyIR;
use crate::ir::{EntityKind, Id};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Per-switch breakdown inside one job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchStats {
    /// Canonical switch identifier (the Id from the IR).
    pub switch_id: String,
    /// Cell this switch belongs to (`meta["cell"]`), or `"?"` if absent.
    pub cell: String,
    /// Rack this switch belongs to (`meta["rack"]`), or `"?"` if absent.
    pub rack: String,
    /// Nodes of *this job* that are under this switch.
    pub nodes: Vec<String>,
    /// Number of nodes of this job under this switch.
    pub node_count: usize,
}

/// Per-group (cell) breakdown inside one job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupStats {
    /// Cell name.
    pub cell: String,
    /// Number of distinct L1 switches used by this job within this cell.
    pub switch_count: usize,
    /// Total nodes of this job within this cell.
    pub node_count: usize,
    /// Per-switch detail, sorted by switch_id.
    pub switches: Vec<SwitchStats>,
}

/// Statistics for a single job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStats {
    /// Total nodes requested.
    pub total_nodes: usize,
    /// Number of distinct cells spanned.
    pub group_count: usize,
    /// Number of distinct L1 switches spanned (across all cells).
    pub total_switch_count: usize,
    /// Per-cell breakdown, sorted by cell name.
    pub groups: Vec<GroupStats>,
    /// Nodes whose L1 switch / cell could not be resolved in the IR.
    pub unresolved_nodes: Vec<String>,
}

/// Top-level statistics for a whole allocation (many jobs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementStats {
    /// Total number of jobs.
    pub job_count: usize,
    /// Total nodes across all jobs.
    pub total_nodes: usize,
    /// Number of distinct cells used across all jobs.
    pub distinct_groups: BTreeSet<String>,
    /// Number of distinct L1 switches used across all jobs.
    pub distinct_switches: BTreeSet<String>,
    /// Per-job breakdown, keyed by job name.
    pub jobs: BTreeMap<String, JobStats>,
}

// ---------------------------------------------------------------------------
// Core computation
// ---------------------------------------------------------------------------

impl PlacementStats {
    /// Compute statistics for `allocation` using topology from `ir`.
    ///
    /// `allocation` maps job-name → list of node hostname strings.
    pub fn compute(ir: &TopologyIR, allocation: &BTreeMap<String, Vec<String>>) -> Self {
        // Build a fast hostname → Id index from the IR once.
        let hostname_index: HashMap<&str, &Id> = ir
            .entities
            .iter()
            .filter(|(_, e)| e.kind == EntityKind::Compute)
            .filter_map(|(id, e)| {
                // Prefer the `hostname` meta field; fall back to the raw Id string.
                let name = e.meta.get("hostname").map(String::as_str).unwrap_or(&id.0);
                Some((name, id))
            })
            .collect();

        let mut all_cells: BTreeSet<String> = BTreeSet::new();
        let mut all_switches: BTreeSet<String> = BTreeSet::new();
        let mut total_nodes = 0usize;

        let jobs: BTreeMap<String, JobStats> = allocation
            .iter()
            .map(|(job_name, hostnames)| {
                let js = compute_job_stats(hostnames, ir, &hostname_index);
                total_nodes += js.total_nodes;
                for g in &js.groups {
                    all_cells.insert(g.cell.clone());
                    for sw in &g.switches {
                        all_switches.insert(sw.switch_id.clone());
                    }
                }
                (job_name.clone(), js)
            })
            .collect();

        Self {
            job_count: jobs.len(),
            total_nodes,
            distinct_groups: all_cells,
            distinct_switches: all_switches,
            jobs,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn compute_job_stats(
    hostnames: &[String],
    ir: &TopologyIR,
    hostname_index: &HashMap<&str, &Id>,
) -> JobStats {
    // Group nodes by (cell, l1_switch_id).
    // key: (cell, switch_id_string, rack_string)
    let mut by_switch: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    let mut unresolved: Vec<String> = Vec::new();

    for hostname in hostnames {
        match resolve_node(hostname, ir, hostname_index) {
            Some((cell, switch_id, rack)) => {
                by_switch
                    .entry((cell, switch_id, rack))
                    .or_default()
                    .push(hostname.clone());
            }
            None => unresolved.push(hostname.clone()),
        }
    }

    // Roll up into groups (cells).
    let mut by_cell: BTreeMap<String, Vec<SwitchStats>> = BTreeMap::new();
    for ((cell, switch_id, rack), nodes) in by_switch {
        let node_count = nodes.len();
        by_cell.entry(cell.clone()).or_default().push(SwitchStats {
            switch_id,
            cell,
            rack,
            node_count,
            nodes,
        });
    }

    // Sort switches within each group by switch_id for determinism.
    for switches in by_cell.values_mut() {
        switches.sort_by(|a, b| a.switch_id.cmp(&b.switch_id));
    }

    let groups: Vec<GroupStats> = by_cell
        .into_iter()
        .map(|(cell, switches)| {
            let node_count = switches.iter().map(|s| s.node_count).sum();
            let switch_count = switches.len();
            GroupStats {
                cell,
                switch_count,
                node_count,
                switches,
            }
        })
        .collect();

    // Sort groups by cell name.
    let mut groups = groups;
    groups.sort_by(|a, b| a.cell.cmp(&b.cell));

    let group_count = groups.len();
    let total_switch_count: usize = groups.iter().map(|g| g.switch_count).sum();
    let total_nodes = hostnames.len();

    JobStats {
        total_nodes,
        group_count,
        total_switch_count,
        groups,
        unresolved_nodes: unresolved,
    }
}

/// Resolve a hostname to `(cell, switch_id, rack)` using the IR.
///
/// Resolution path:
///   1. Look up the node Id via `hostname_index`.
///   2. Find the L1 switch that contains it via `ir.contains`.
///   3. Read `cell` and `rack` from the switch's `meta`.
fn resolve_node(
    hostname: &str,
    ir: &TopologyIR,
    hostname_index: &HashMap<&str, &Id>,
) -> Option<(String, String, String)> {
    let node_id = hostname_index.get(hostname)?;

    // Walk `ir.contains` to find the switch that owns this node.
    let (switch_id, switch_entity) = ir
        .contains
        .iter()
        .find(|(_, children)| children.contains(node_id))
        .and_then(|(parent_id, _)| {
            ir.entities
                .get(parent_id)
                .map(|e| (parent_id, e))
        })?;

    // Only count L1 (level 0) switches; skip higher-level aggregation switches.
    if !matches!(switch_entity.kind, EntityKind::Switch { level: Some(0) }) {
        return None;
    }

    let cell = switch_entity
        .meta
        .get("cell")
        .cloned()
        .unwrap_or_else(|| "ungrouped".into());
    let rack = switch_entity
        .meta
        .get("rack")
        .cloned()
        .unwrap_or_else(|| "?".into());

    Some((cell, switch_id.0.clone(), rack))
}