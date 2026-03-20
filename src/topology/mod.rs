//! Generic topology-parsing infrastructure.
//!
//! Each supercomputer backend implements [`TopologyParser`].  The blanket
//! helpers in this module (`enrich_with_sinfo`, `filter_nodes`) are
//! system-agnostic and can be called by any backend after it has produced a
//! basic [`TopologyIR`].

use crate::ir::topology_ir::TopologyIR;
use crate::ir::EntityKind;
use crate::ir::Id;
use crate::parsers::sinfo::{self, index_by_hostname, NodeInfo, NodeState};
use crate::parsers::slurm::NodeListParseError;

pub mod alps;
pub mod jupiter;
pub mod leonardo;

#[derive(Clone)]
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

fn resolve_sinfo_raw(source: Option<SinfoSource>) -> Result<Option<String>, NodeListParseError> {
    match source {
        None => Ok(None),
        Some(SinfoSource::Command) => Ok(Some(sinfo::from_sinfo_command_raw()?)),
        Some(SinfoSource::File(path)) => Ok(Some(sinfo::from_sinfo_file_raw(path)?)),
    }
}

// ---------------------------------------------------------------------------
// Options shared across all parsers
// ---------------------------------------------------------------------------

/// Controls which nodes are retained after parsing.
#[derive(Debug, Clone)]
pub struct NodeFilterOptions {
    /// If `true`, nodes whose state is Draining, Drained, or Down are removed.
    /// Default: `true`.
    pub remove_unavailable: bool,
}

impl Default for NodeFilterOptions {
    fn default() -> Self {
        Self {
            remove_unavailable: true,
        }
    }
}

// ---------------------------------------------------------------------------
// The trait every supercomputer backend must implement
// ---------------------------------------------------------------------------

/// A backend that knows how to turn raw `scontrol show topology` text (and
/// optionally `sinfo` text) into a [`TopologyIR`].
///
/// # Required methods
///
/// * [`parse_topology_raw`] – convert `scontrol` output into a base IR
///   containing nodes, switches, intra-cell links, and *known* inter-switch
///   links.
/// * [`enrich_inter_cell_links`] – add the inter-cell links that are absent
///   from `scontrol` output but are known from hardware documentation.
///
/// # Provided methods
///
/// * [`enrich_with_sinfo`] – attach `partition` / `state` metadata from
///   `sinfo` records to every compute node already in the IR.
/// * [`apply_node_filter`] – remove nodes (and their dangling topology) based
///   on [`NodeFilterOptions`].
/// * [`from_raw`] / [`from_file`] / [`from_scontrol`] – full pipeline helpers.
pub trait TopologyParser {
    // ------------------------------------------------------------------
    // Backend-specific
    // ------------------------------------------------------------------

    /// Parse raw `scontrol show topology` output into a base [`TopologyIR`].
    ///
    /// At this stage inter-cell links may be absent; call
    /// [`enrich_inter_cell_links`] afterwards.
    fn parse_topology_raw(&self, raw: &str) -> Result<TopologyIR, NodeListParseError>;

    /// Inject inter-cell links that are not present in `scontrol` output.
    ///
    /// Implementations derive cell membership from switch naming conventions
    /// and add weighted links accordingly.
    fn enrich_inter_cell_links(&self, ir: &mut TopologyIR);

    fn enrich_node_info(&self, ir: &mut TopologyIR, node_infos: &[NodeInfo]);

    // ------------------------------------------------------------------
    // Provided: sinfo enrichment
    // ------------------------------------------------------------------

    /// Attach `partition` and `state` metadata to every compute [`Entity`]
    /// that is found in `node_infos`.
    ///
    /// * `partition` is stored as `meta["partition"]`.
    /// * `state` is stored as `meta["state"]` (the canonical lowercase string).
    ///
    /// A node that appears in multiple partitions gets all partition names
    /// stored as a comma-separated value under `"partition"`.
    fn enrich_with_sinfo(&self, ir: &mut TopologyIR, node_infos: Vec<NodeInfo>) {
        let index = index_by_hostname(node_infos);

        for entity in ir.entities.values_mut() {
            if !matches!(entity.kind, EntityKind::Compute) {
                continue;
            }
            let hostname = &entity.id.0;
            if let Some(infos) = index.get(hostname) {
                // Collect unique partitions (a node may appear multiple times
                // in the same partition with different states, e.g. mix + drng).
                let partitions: Vec<String> = {
                    let mut seen = std::collections::HashSet::new();
                    infos
                        .iter()
                        .map(|i| i.partition.clone())
                        .filter(|p| seen.insert(p.clone()))
                        .collect()
                };
                entity.meta.insert("partition".into(), partitions.join(","));

                // Use the "worst" state if the node appears multiple times.
                let state = infos
                    .iter()
                    .map(|i| &i.state)
                    .max_by_key(|s| state_severity(s))
                    .unwrap(); // infos is non-empty
                entity
                    .meta
                    .insert("state".into(), state_to_str(state).to_string());
            }
        }
    }

    // ------------------------------------------------------------------
    // Provided: node filtering
    // ------------------------------------------------------------------

    /// Remove nodes from the IR according to `opts`, then prune any switches
    /// that have become empty (no remaining compute descendants).
    fn apply_node_filter(&self, ir: TopologyIR, opts: &NodeFilterOptions) -> TopologyIR {
        if !opts.remove_unavailable {
            return ir;
        }

        // Collect IDs of unavailable compute nodes.
        let remove: Vec<Id> = ir
            .entities
            .values()
            .filter(|e| {
                if !matches!(e.kind, EntityKind::Compute) {
                    return false;
                }
                e.meta
                    .get("state")
                    .map(|s| {
                        let state = crate::parsers::sinfo::NodeState::from_str(s);
                        state.is_unavailable()
                    })
                    .unwrap_or(false)
            })
            .map(|e| e.id.clone())
            .collect();

        if remove.is_empty() {
            return ir;
        }

        ir.filter_remove_ids(&remove)
    }

    // ------------------------------------------------------------------
    // Provided: full pipeline
    // ------------------------------------------------------------------

    /// Full pipeline from raw strings.
    ///
    /// 1. `parse_topology_raw`
    /// 2. `enrich_inter_cell_links`
    /// 3. (optional) `enrich_with_sinfo`
    /// 4. `apply_node_filter`
    fn build(
        &self,
        topology_raw: &str,
        sinfo_infos: Option<Vec<NodeInfo>>,
        opts: &NodeFilterOptions,
    ) -> Result<TopologyIR, NodeListParseError> {
        let mut ir = self.parse_topology_raw(topology_raw)?;
        self.enrich_inter_cell_links(&mut ir);
        if let Some(infos) = sinfo_infos {
            self.enrich_with_sinfo(&mut ir, infos);
        }
        Ok(self.apply_node_filter(ir, opts))
    }
}

// ---------------------------------------------------------------------------
// Helpers for TopologyIR that are useful across backends
// ---------------------------------------------------------------------------

/// Insert (or update) a metadata key on an entity, if it exists.
pub fn set_meta(ir: &mut TopologyIR, id: &Id, key: &str, value: String) {
    if let Some(entity) = ir.entities.get_mut(id) {
        entity.meta.insert(key.to_string(), value);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn state_severity(s: &NodeState) -> u8 {
    match s {
        NodeState::Down => 4,
        NodeState::Drained => 3,
        NodeState::Draining => 2,
        NodeState::Mixed => 1,
        _ => 0,
    }
}

fn state_to_str(s: &NodeState) -> &'static str {
    match s {
        NodeState::Allocated => "allocated",
        NodeState::Mixed => "mixed",
        NodeState::Idle => "idle",
        NodeState::Draining => "draining",
        NodeState::Drained => "drained",
        NodeState::Down => "down",
        NodeState::Other(_) => "other",
    }
}
