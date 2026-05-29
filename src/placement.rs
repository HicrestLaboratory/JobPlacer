// placement.rs
// Handles multi-parent nodes, proper round-robin depletion, and placement strategies.

use std::collections::{BTreeMap, HashMap, HashSet};

use log::info;
use rand::prelude::*;
use serde::{Deserialize, Serialize};

use crate::graph::display::Allocations;
use crate::ir::topology_ir::TopologyIR;
use crate::ir::EntityKind;
use crate::ir::Id;

// ---------------------------------------------------------------------------
// Topology Normalization: Handle multi-parent nodes
// ---------------------------------------------------------------------------

/// Normalize topology when nodes have multiple parents (switches).
/// Creates synthetic L1 switches that collapse the multiple parents into one.
fn normalize_multiparent_topology(ir: &TopologyIR) -> TopologyIR {
    let mut node_to_parents: HashMap<Id, Vec<Id>> = HashMap::new();

    for (parent, children) in &ir.contains {
        for child in children {
            node_to_parents
                .entry(child.clone())
                .or_insert_with(Vec::new)
                .push(parent.clone());
        }
    }

    let mut parent_sets: HashMap<Vec<Id>, Vec<Id>> = HashMap::new();

    for (node, parents) in node_to_parents.iter() {
        if parents.len() > 1 {
            let mut sorted_parents = parents.clone();
            sorted_parents.sort_by(|a, b| a.0.cmp(&b.0));
            parent_sets
                .entry(sorted_parents)
                .or_insert_with(Vec::new)
                .push(node.clone());
        }
    }

    if parent_sets.is_empty() {
        return ir.clone();
    }

    let mut new_entities = ir.entities.clone();
    let mut new_contains = ir.contains.clone();

    for (parent_set, nodes_in_group) in parent_sets {
        let synthetic_id = if let Some(_first_parent) = parent_set.first() {
            let parent_hash = parent_set
                .iter()
                .map(|p| p.0.as_str())
                .collect::<Vec<_>>()
                .join("_");
            Id(format!("__synthetic_l1_{}", parent_hash))
        } else {
            continue;
        };

        if let Some(first_parent) = parent_set.first() {
            if let Some(parent_entity) = ir.entities.get(first_parent) {
                let mut synthetic_entity = parent_entity.clone();
                synthetic_entity.id = synthetic_id.clone();
                synthetic_entity
                    .meta
                    .insert("__synthetic".to_string(), "true".to_string());
                new_entities.insert(synthetic_id.clone(), synthetic_entity);
            }
        }

        for parent in &parent_set {
            if let Some(children) = new_contains.get_mut(parent) {
                children.retain(|child| !nodes_in_group.contains(child));
            }
        }

        new_contains
            .entry(synthetic_id)
            .or_insert_with(Vec::new)
            .extend(nodes_in_group);
    }

    new_contains.retain(|_, children| !children.is_empty());

    let mut normalized = ir.clone();
    normalized.entities = new_entities;
    normalized.contains = new_contains;
    normalized
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementClass {
    IntraL1,
    IntraGroup,
    InterGroup,
    IntraGroupSameL1(usize),
    InterGroupSameL1(usize),
}

impl Serialize for PlacementClass {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let txt = match self {
            PlacementClass::IntraL1 => "intra-l1".to_owned(),
            PlacementClass::IntraGroup => "intra-group".to_owned(),
            PlacementClass::InterGroup => "inter-group".to_owned(),
            PlacementClass::IntraGroupSameL1(n) => format!("intra-group-same-l1-{n}"),
            PlacementClass::InterGroupSameL1(n) => format!("inter-group-same-l1-{n}"),
        };
        s.serialize_str(&txt)
    }
}

impl<'de> Deserialize<'de> for PlacementClass {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        match raw.as_str() {
            "intra-l1" => return Ok(PlacementClass::IntraL1),
            "intra-group" => return Ok(PlacementClass::IntraGroup),
            "inter-group" => return Ok(PlacementClass::InterGroup),
            _ => {}
        }
        if let Some(n_str) = raw.strip_prefix("intra-group-same-l1-") {
            let n = n_str.parse::<usize>().map_err(serde::de::Error::custom)?;
            return Ok(PlacementClass::IntraGroupSameL1(n));
        }
        if let Some(n_str) = raw.strip_prefix("inter-group-same-l1-") {
            let n = n_str.parse::<usize>().map_err(serde::de::Error::custom)?;
            return Ok(PlacementClass::InterGroupSameL1(n));
        }
        Err(serde::de::Error::unknown_variant(
            &raw,
            &[
                "intra-l1",
                "intra-group",
                "inter-group",
                "intra-group-same-l1-<n>",
                "inter-group-same-l1-<n>",
            ],
        ))
    }
}

// ---------------------------------------------------------------------------
// Placement strategy
// ---------------------------------------------------------------------------

/// Controls how aggressively the placer relaxes constraints when strict
/// placement fails.
///
/// Pass this to `Placer::place_with_strategy`. The default (used by
/// `Placer::place`) is `Strict`.
///
/// Hierarchy of relaxations, from tightest to loosest:
///
/// ```text
/// Strict      – honour every constraint exactly; fail if infeasible.
/// Relaxed     – IntraGroup may spill across cells when no single cell is large
///               enough (becomes effectively InterGroup). All other constraints
///               stay exact.
/// BestEffort  – additionally allows IntraL1 to spill to IntraGroup, and
///               IntraGroupSameL1 / InterGroupSameL1 to drop the block-alignment
///               requirement when no aligned solution exists. The placement_class
///               recorded in the output reflects what was actually achieved.
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementStrategy {
    #[default]
    Strict,
    Relaxed,
    BestEffort,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobRequest {
    pub nodes: usize,
    pub placement_class: PlacementClass,
}

/// A successfully placed job: the exact compute node IDs assigned and the
/// placement class that was *actually achieved* (may differ from the request
/// when strategy == Relaxed or BestEffort).
#[derive(Debug, Clone, Serialize)]
pub struct JobPlacement {
    pub nodes: Vec<String>,
    /// The placement class as requested.
    pub placement_class: String,
    /// The class actually achieved (same as `placement_class` under Strict).
    pub achieved_class: String,
}

/// Output of a full placement attempt.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status")]
pub enum PlacementResult {
    #[serde(rename = "ok")]
    Ok {
        placements: BTreeMap<String, JobPlacement>,
    },
    #[serde(rename = "infeasible")]
    Infeasible { reason: String },
}

pub fn filter_ir_by_allocations(ir: &TopologyIR, allocations: &Allocations) -> TopologyIR {
    let allocated_nodes: HashSet<Id> = allocations
        .values()
        .flatten()
        .map(|s| Id(s.clone()))
        .collect();

    let active_l1s: HashSet<Id> = ir
        .entities
        .values()
        .filter(|e| matches!(e.kind, EntityKind::Switch { level: Some(0) }))
        .filter(|e| {
            ir.contains
                .get(&e.id)
                .map(|children| children.iter().any(|c| allocated_nodes.contains(c)))
                .unwrap_or(false)
        })
        .map(|e| e.id.clone())
        .collect();

    let keep: Vec<Id> = ir
        .entities
        .keys()
        .filter(|id| {
            allocated_nodes.contains(id)
                || active_l1s.contains(id)
                || matches!(
                    ir.entities.get(id),
                    Some(e) if matches!(e.kind, EntityKind::Switch { level: Some(1) })
                )
        })
        .cloned()
        .collect();

    ir.filter_by_ids(&keep)
}

// ---------------------------------------------------------------------------
// Topology views
// ---------------------------------------------------------------------------

struct TopoView {
    /// cell_id → rack_id → l1_switch_id → [compute_node_id]
    cells: BTreeMap<String, BTreeMap<String, BTreeMap<Id, Vec<Id>>>>,
}

impl TopoView {
    fn build(ir: &TopologyIR) -> Self {
        let mut cells: BTreeMap<String, BTreeMap<String, BTreeMap<Id, Vec<Id>>>> = BTreeMap::new();

        for entity in ir.entities.values() {
            if !matches!(entity.kind, EntityKind::Switch { level: Some(0) }) {
                continue;
            }
            let cell = entity
                .meta
                .get("cell")
                .cloned()
                .unwrap_or_else(|| "ungrouped".into());
            let rack = entity
                .meta
                .get("rack")
                .cloned()
                .unwrap_or_else(|| "?".into());

            let compute: Vec<Id> = ir
                .contains
                .get(&entity.id)
                .map(|children| {
                    children
                        .iter()
                        .filter(|id| {
                            matches!(
                                ir.entities.get(id),
                                Some(e) if matches!(e.kind, EntityKind::Compute)
                            )
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();

            cells
                .entry(cell)
                .or_default()
                .entry(rack)
                .or_default()
                .insert(entity.id.clone(), compute);
        }

        Self { cells }
    }

    fn nodes_in_l1<'a>(&'a self, cell: &str, rack: &str, l1: &Id) -> Vec<&'a Id> {
        self.cells
            .get(cell)
            .and_then(|r| r.get(rack))
            .and_then(|l| l.get(l1))
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    fn nodes_in_cell<'a>(&'a self, cell: &str) -> Vec<&'a Id> {
        self.cells
            .get(cell)
            .map(|racks| {
                racks
                    .values()
                    .flat_map(|l1s| l1s.values().flat_map(|nodes| nodes.iter()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn free_per_l1_in_cell(&self, cell: &str, used: &HashSet<Id>) -> Vec<usize> {
        self.cells
            .get(cell)
            .map(|racks| {
                racks
                    .values()
                    .flat_map(|l1s| l1s.values())
                    .map(|nodes| nodes.iter().filter(|id| !used.contains(*id)).count())
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Placement engine
// ---------------------------------------------------------------------------

pub struct Placer {
    ir: TopologyIR,
    view: TopoView,
    rng: SmallRng,
}

impl Placer {
    pub fn new(ir: &TopologyIR, seed: u64) -> Self {
        let normalized_ir = normalize_multiparent_topology(ir);
        Self {
            view: TopoView::build(&normalized_ir),
            ir: normalized_ir,
            rng: SmallRng::seed_from_u64(seed),
        }
    }

    pub fn change_seed(&mut self, new_seed: u64) {
        self.rng = SmallRng::seed_from_u64(new_seed);
    }

    /// Attempt placement with the default (Strict) strategy.
    pub fn place(&mut self, jobs: &BTreeMap<String, JobRequest>) -> PlacementResult {
        self.place_with_strategy(jobs, PlacementStrategy::Strict)
    }

    /// Attempt placement with a caller-specified fallback strategy.
    pub fn place_with_strategy(
        &mut self,
        jobs: &BTreeMap<String, JobRequest>,
        strategy: PlacementStrategy,
    ) -> PlacementResult {
        // Sort: tightest constraint first, then larger jobs within same class.
        let constraint_rank = |pc: &PlacementClass| match pc {
            PlacementClass::IntraL1 => 0usize,
            PlacementClass::IntraGroupSameL1(_) => 1,
            PlacementClass::InterGroupSameL1(_) => 2,
            PlacementClass::IntraGroup => 3,
            PlacementClass::InterGroup => 4,
        };
        let mut job_order: Vec<(&String, &JobRequest)> = jobs.iter().collect();
        job_order.sort_by(|a, b| {
            constraint_rank(&a.1.placement_class)
                .cmp(&constraint_rank(&b.1.placement_class))
                .then_with(|| b.1.nodes.cmp(&a.1.nodes))
        });

        let mut used: HashSet<Id> = HashSet::new();
        let mut placements: BTreeMap<String, JobPlacement> = BTreeMap::new();

        let total_requested: usize = jobs.values().map(|j| j.nodes).sum();
        let total_available: usize = self
            .ir
            .entities
            .iter()
            .filter(|(_, e)| e.kind == EntityKind::Compute)
            .count();

        info!(
            "Placing {} jobs requiring {} nodes total ({} available) [strategy={:?}]",
            jobs.len(),
            total_requested,
            total_available,
            strategy,
        );

        for (job_name, req) in &job_order {
            match self.place_one_with_strategy(req, &used, strategy) {
                Some((assigned, achieved_class)) => {
                    for id in &assigned {
                        used.insert(id.clone());
                    }
                    placements.insert(
                        (*job_name).clone(),
                        JobPlacement {
                            nodes: assigned.into_iter().map(|id| id.0).collect(),
                            placement_class: format!("{:?}", req.placement_class),
                            achieved_class,
                        },
                    );
                }
                None => {
                    let debug = self.failure_debug(req, &used);
                    return PlacementResult::Infeasible {
                        reason: format!(
                            "cannot place job '{}': need {} nodes with class {:?} \
                             (strategy={:?}). {}",
                            job_name, req.nodes, req.placement_class, strategy, debug
                        ),
                    };
                }
            }
        }

        assert_eq!(
            total_requested,
            placements.values().map(|p| p.nodes.len()).sum::<usize>(),
            "placed node count mismatch: requested {total_requested}"
        );

        let mut seen: HashSet<&str> = HashSet::new();
        for (job_name, placement) in &placements {
            for node in &placement.nodes {
                assert!(
                    seen.insert(node.as_str()),
                    "node '{node}' appears in job '{job_name}' but was already assigned"
                );
            }
        }

        PlacementResult::Ok { placements }
    }

    // -----------------------------------------------------------------------
    // Failure diagnostics
    // -----------------------------------------------------------------------

    fn failure_debug(&self, req: &JobRequest, used: &HashSet<Id>) -> String {
        let total_free: usize = self
            .ir
            .entities
            .iter()
            .filter(|(id, e)| e.kind == EntityKind::Compute && !used.contains(*id))
            .count();

        let mut lines: Vec<String> = vec![format!("Cluster-wide free nodes: {}.", total_free)];

        for (cell, _racks) in &self.view.cells {
            let per_l1: Vec<usize> = self.view.free_per_l1_in_cell(cell, used);
            let cell_free: usize = per_l1.iter().sum();
            let l1s_with_any: usize = per_l1.iter().filter(|&&c| c > 0).count();

            lines.push(format!(
                "  Cell '{}': {} free nodes across {} L1s (per-L1 free: [{}]).",
                cell,
                cell_free,
                l1s_with_any,
                per_l1
                    .iter()
                    .filter(|&&c| c > 0)
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));

            let block_size = match req.placement_class {
                PlacementClass::IntraGroupSameL1(bs) | PlacementClass::InterGroupSameL1(bs) => {
                    Some(bs)
                }
                _ => None,
            };
            if let Some(bs) = block_size {
                let slots: usize = per_l1.iter().map(|&c| c / bs).sum();
                let l1s_with_slot: usize = per_l1.iter().filter(|&&c| c >= bs).count();
                lines.push(format!(
                    "    Block-size {}: {} L1s with ≥1 slot, {} total slots available.",
                    bs, l1s_with_slot, slots
                ));
            }
        }

        match &req.placement_class {
            PlacementClass::IntraL1 => {
                let best = self
                    .view
                    .cells
                    .iter()
                    .flat_map(|(cell, racks)| {
                        racks.iter().flat_map(move |(rack, l1s)| {
                            l1s.keys().map(move |l1| {
                                self.view
                                    .nodes_in_l1(cell, rack, l1)
                                    .iter()
                                    .filter(|id| !used.contains(*id))
                                    .count()
                            })
                        })
                    })
                    .max()
                    .unwrap_or(0);
                lines.push(format!(
                    "IntraL1 needs {} nodes under one L1; largest free L1 has {}.",
                    req.nodes, best
                ));
            }
            PlacementClass::IntraGroup => {
                let best_cell_free = self
                    .view
                    .cells
                    .keys()
                    .map(|cell| {
                        self.view
                            .nodes_in_cell(cell)
                            .iter()
                            .filter(|id| !used.contains(*id))
                            .count()
                    })
                    .max()
                    .unwrap_or(0);
                lines.push(format!(
                    "IntraGroup needs {} nodes within one cell spanning ≥2 L1s; \
                     largest cell has {} free nodes.",
                    req.nodes, best_cell_free
                ));
            }
            PlacementClass::InterGroup => {
                let num_cells_with_free = self
                    .view
                    .cells
                    .keys()
                    .filter(|cell| {
                        self.view
                            .nodes_in_cell(cell)
                            .iter()
                            .any(|id| !used.contains(*id))
                    })
                    .count();
                lines.push(format!(
                    "InterGroup needs {} nodes spanning ≥2 cells; {} cells have free nodes.",
                    req.nodes, num_cells_with_free
                ));
            }
            PlacementClass::IntraGroupSameL1(bs) => {
                lines.push(format!(
                    "IntraGroupSameL1({}) needs {} nodes ({} blocks of {}) within one cell \
                     across ≥2 L1s.",
                    bs,
                    req.nodes,
                    req.nodes / bs,
                    bs
                ));
            }
            PlacementClass::InterGroupSameL1(bs) => {
                lines.push(format!(
                    "InterGroupSameL1({}) needs {} nodes ({} blocks of {}) spanning ≥2 cells.",
                    bs,
                    req.nodes,
                    req.nodes / bs,
                    bs
                ));
            }
        }

        lines.join(" ")
    }

    // -----------------------------------------------------------------------
    // Single-job placement (strategy-aware)
    // -----------------------------------------------------------------------

    /// Returns `(assigned_nodes, achieved_class_label)` on success.
    fn place_one_with_strategy(
        &mut self,
        req: &JobRequest,
        used: &HashSet<Id>,
        strategy: PlacementStrategy,
    ) -> Option<(Vec<Id>, String)> {
        // Always try strict first.
        if let Some(nodes) = self.place_one_inner(req, used) {
            return Some((nodes, format!("{:?}", req.placement_class)));
        }

        // Under Strict, no fallback.
        if strategy == PlacementStrategy::Strict {
            return None;
        }

        // ── Relaxed / BestEffort fallbacks ──────────────────────────────────

        match &req.placement_class {
            // IntraGroup → try InterGroup (spill across cells)
            PlacementClass::IntraGroup => {
                info!(
                    "IntraGroup strict failed for {} nodes; falling back to InterGroup",
                    req.nodes
                );
                let fallback = JobRequest {
                    nodes: req.nodes,
                    placement_class: PlacementClass::InterGroup,
                };
                self.place_one_inner(&fallback, used)
                    .map(|nodes| (nodes, "InterGroup(relaxed-from-IntraGroup)".to_string()))
            }

            // IntraGroupSameL1 → try InterGroupSameL1 (same blocks, cross-cell)
            PlacementClass::IntraGroupSameL1(bs) => {
                let bs = *bs;
                info!(
                    "IntraGroupSameL1({}) strict failed for {} nodes; \
                     falling back to InterGroupSameL1({})",
                    bs, req.nodes, bs
                );
                let fallback = JobRequest {
                    nodes: req.nodes,
                    placement_class: PlacementClass::InterGroupSameL1(bs),
                };
                if let Some(nodes) = self.place_one_inner(&fallback, used) {
                    return Some((
                        nodes,
                        format!("InterGroupSameL1({bs})(relaxed-from-IntraGroupSameL1({bs}))"),
                    ));
                }

                // BestEffort: drop block alignment entirely → InterGroup
                if strategy == PlacementStrategy::BestEffort {
                    info!(
                        "InterGroupSameL1({}) also failed; dropping block alignment",
                        bs
                    );
                    let fallback2 = JobRequest {
                        nodes: req.nodes,
                        placement_class: PlacementClass::InterGroup,
                    };
                    self.place_one_inner(&fallback2, used).map(|nodes| {
                        (
                            nodes,
                            format!("InterGroup(best-effort-from-IntraGroupSameL1({bs}))"),
                        )
                    })
                } else {
                    None
                }
            }

            // InterGroupSameL1 → BestEffort: drop block alignment → InterGroup
            PlacementClass::InterGroupSameL1(bs) if strategy == PlacementStrategy::BestEffort => {
                let bs = *bs;
                info!(
                    "InterGroupSameL1({}) strict failed for {} nodes; \
                     dropping block alignment (BestEffort)",
                    bs, req.nodes
                );
                let fallback = JobRequest {
                    nodes: req.nodes,
                    placement_class: PlacementClass::InterGroup,
                };
                self.place_one_inner(&fallback, used).map(|nodes| {
                    (
                        nodes,
                        format!("InterGroup(best-effort-from-InterGroupSameL1({bs}))"),
                    )
                })
            }

            // IntraL1 → BestEffort: relax to IntraGroup, then InterGroup
            PlacementClass::IntraL1 if strategy == PlacementStrategy::BestEffort => {
                info!(
                    "IntraL1 strict failed for {} nodes; trying IntraGroup (BestEffort)",
                    req.nodes
                );
                let fallback_ig = JobRequest {
                    nodes: req.nodes,
                    placement_class: PlacementClass::IntraGroup,
                };
                if let Some(nodes) = self.place_one_inner(&fallback_ig, used) {
                    return Some((nodes, "IntraGroup(best-effort-from-IntraL1)".to_string()));
                }
                let fallback_inter = JobRequest {
                    nodes: req.nodes,
                    placement_class: PlacementClass::InterGroup,
                };
                self.place_one_inner(&fallback_inter, used)
                    .map(|nodes| (nodes, "InterGroup(best-effort-from-IntraL1)".to_string()))
            }

            // All other combinations: no defined fallback.
            _ => None,
        }
    }

    fn place_one_inner(&mut self, req: &JobRequest, used: &HashSet<Id>) -> Option<Vec<Id>> {
        match &req.placement_class {
            PlacementClass::IntraL1 => self.place_intra_l1(req.nodes, used),
            PlacementClass::IntraGroup => self.place_intra_group(req.nodes, used),
            PlacementClass::InterGroup => self.place_inter_group(req.nodes, used),
            PlacementClass::IntraGroupSameL1(bs) => {
                self.place_intra_group_same_l1(req.nodes, *bs, used)
            }
            PlacementClass::InterGroupSameL1(bs) => {
                self.place_inter_group_same_l1(req.nodes, *bs, used)
            }
        }
    }

    // -----------------------------------------------------------------------
    // IntraL1
    // -----------------------------------------------------------------------

    fn place_intra_l1(&mut self, n: usize, used: &HashSet<Id>) -> Option<Vec<Id>> {
        let mut candidates: Vec<(String, String, Id)> = self
            .view
            .cells
            .iter()
            .flat_map(|(cell, racks)| {
                racks.iter().flat_map(move |(rack, l1s)| {
                    l1s.keys()
                        .map(move |l1| (cell.clone(), rack.clone(), l1.clone()))
                })
            })
            .filter(|(cell, rack, l1)| {
                self.view
                    .nodes_in_l1(cell, rack, l1)
                    .into_iter()
                    .filter(|id| !used.contains(*id))
                    .count()
                    >= n
            })
            .collect();

        candidates.shuffle(&mut self.rng);
        let (cell, rack, l1) = candidates.into_iter().next()?;

        let mut free: Vec<Id> = self
            .view
            .nodes_in_l1(&cell, &rack, &l1)
            .into_iter()
            .filter(|id| !used.contains(*id))
            .cloned()
            .collect();
        free.shuffle(&mut self.rng);
        Some(free.into_iter().take(n).collect())
    }

    // -----------------------------------------------------------------------
    // IntraGroup
    // -----------------------------------------------------------------------

    /// All nodes within one cell, spanning >1 L1.
    ///
    /// Key fix vs. original: the cell is only attempted if it has enough free
    /// nodes in total *and* across ≥2 L1 switches. If no single cell qualifies,
    /// we return None (the strategy layer will decide whether to widen scope).
    fn place_intra_group(&mut self, n: usize, used: &HashSet<Id>) -> Option<Vec<Id>> {
        // Sort cells by descending free-node count so we try the richest cell
        // first — maximises the chance of finding a valid assignment on the
        // first attempt and reduces wasted RNG draws.
        let mut cell_free_counts: Vec<(String, usize)> = self
            .view
            .cells
            .keys()
            .map(|cell| {
                let free = self
                    .view
                    .nodes_in_cell(cell)
                    .into_iter()
                    .filter(|id| !used.contains(*id))
                    .count();
                (cell.clone(), free)
            })
            .filter(|(_, free)| *free >= n)
            .collect();

        // Shuffle first for seed-based randomness among equally-rich cells.
        cell_free_counts.shuffle(&mut self.rng);
        cell_free_counts.sort_by(|a, b| b.1.cmp(&a.1));

        for (cell, _) in &cell_free_counts {
            let mut selected = self.pick_spanning_l1(cell, n, used);
            if selected.is_none() && n == 1 {
                let mut pool: Vec<Id> = self
                    .view
                    .nodes_in_cell(cell)
                    .into_iter()
                    .filter(|id| !used.contains(*id))
                    .cloned()
                    .collect();
                pool.shuffle(&mut self.rng);
                selected = pool.into_iter().next().map(|id| vec![id]);
            }
            if selected.is_some() {
                return selected;
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // InterGroup
    // -----------------------------------------------------------------------

    /// Nodes must span ≥2 distinct cells.
    ///
    /// Key fixes vs. original:
    /// 1. We explicitly reserve capacity from a second cell before filling
    ///    from the first, guaranteeing the ≥2-cell constraint is satisfied
    ///    even when one cell dominates in free-node count.
    /// 2. Exhausted pools are removed promptly so the round-robin index stays
    ///    valid.
    fn place_inter_group(&mut self, n: usize, used: &HashSet<Id>) -> Option<Vec<Id>> {
        let mut cell_names: Vec<String> = self.view.cells.keys().cloned().collect();
        cell_names.shuffle(&mut self.rng);

        // Build per-cell free-node pools.
        let mut per_cell: Vec<(String, Vec<Id>)> = cell_names
            .into_iter()
            .map(|cell| {
                let mut free: Vec<Id> = self
                    .view
                    .nodes_in_cell(&cell)
                    .into_iter()
                    .filter(|id| !used.contains(*id))
                    .cloned()
                    .collect();
                free.shuffle(&mut self.rng);
                (cell, free)
            })
            .filter(|(_, free)| !free.is_empty())
            .collect();

        let total_free: usize = per_cell.iter().map(|(_, v)| v.len()).sum();
        if total_free < n || per_cell.len() < 2 {
            return None;
        }

        // Sort descending by pool size (richest cell first) after shuffle so
        // ties are broken randomly.
        per_cell.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        // Reserve at least 1 node for a non-dominant cell to guarantee ≥2 cells.
        // Cap the dominant cell's contribution to n-1 when it alone could supply
        // all n nodes.
        let dominant_cap = if per_cell[0].1.len() >= n { n - 1 } else { per_cell[0].1.len() };

        let mut result: Vec<Id> = Vec::with_capacity(n);
        let mut cells_used: HashSet<String> = HashSet::new();

        // Fill from dominant cell up to its cap.
        let (dominant_cell, dominant_pool) = &mut per_cell[0];
        let take = dominant_cap.min(n);
        result.extend(dominant_pool.drain(..take));
        if !result.is_empty() {
            cells_used.insert(dominant_cell.clone());
        }

        // Round-robin across remaining cells to fill the rest.
        let mut ci = 1usize;
        while result.len() < n {
            // Skip index 0 (dominant, already drained to its cap).
            let remaining_cells = per_cell.len() - 1;
            if remaining_cells == 0 {
                break;
            }
            let idx = 1 + (ci % remaining_cells);
            let (cell, pool) = &mut per_cell[idx];
            if let Some(id) = pool.pop() {
                cells_used.insert(cell.clone());
                result.push(id);
            } else {
                // Pool exhausted — remove it.
                per_cell.remove(idx);
                // Don't advance ci; the next iteration will land on the next pool.
                continue;
            }
            ci += 1;
        }

        if result.len() == n && cells_used.len() >= 2 {
            Some(result)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // IntraGroupSameL1
    // -----------------------------------------------------------------------

    fn place_intra_group_same_l1(
        &mut self,
        total: usize,
        block_size: usize,
        used: &HashSet<Id>,
    ) -> Option<Vec<Id>> {
        if block_size == 0 || total % block_size != 0 {
            return None;
        }
        let num_blocks = total / block_size;

        let mut cell_names: Vec<String> = self.view.cells.keys().cloned().collect();
        cell_names.shuffle(&mut self.rng);

        for cell in &cell_names {
            if let Some(result) =
                self.try_intra_group_same_l1_in_cell(cell, total, block_size, num_blocks, used)
            {
                return Some(result);
            }
        }
        None
    }

    fn try_intra_group_same_l1_in_cell(
        &mut self,
        cell: &str,
        total: usize,
        block_size: usize,
        num_blocks: usize,
        used: &HashSet<Id>,
    ) -> Option<Vec<Id>> {
        let racks = self.view.cells.get(cell)?;

        let mut l1_pools: Vec<Vec<Id>> = racks
            .values()
            .flat_map(|l1s| l1s.values())
            .map(|nodes| {
                nodes
                    .iter()
                    .filter(|id| !used.contains(*id))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .filter(|pool| pool.len() >= block_size)
            .collect();

        if l1_pools.len() < 2 {
            return None;
        }

        let total_slots: usize = l1_pools.iter().map(|p| p.len() / block_size).sum();
        if total_slots < num_blocks {
            return None;
        }

        l1_pools.shuffle(&mut self.rng);
        for pool in &mut l1_pools {
            pool.shuffle(&mut self.rng);
        }
        l1_pools.sort_by(|a, b| b.len().cmp(&a.len()));

        let mut result: Vec<Id> = Vec::with_capacity(total);
        let mut blocks_placed = 0usize;
        let mut blocks_from: Vec<usize> = vec![0; l1_pools.len()];

        let top_cap = l1_pools[0].len() / block_size;
        let max_from_top = if top_cap >= num_blocks {
            num_blocks - 1
        } else {
            top_cap
        };

        for (i, pool) in l1_pools.iter_mut().enumerate() {
            if blocks_placed >= num_blocks {
                break;
            }
            let cap = pool.len() / block_size;
            let limit = if i == 0 { max_from_top } else { cap };
            let take = limit.min(num_blocks - blocks_placed);
            for _ in 0..take {
                if pool.len() < block_size {
                    break;
                }
                let drain_start = pool.len() - block_size;
                let block: Vec<Id> = pool.drain(drain_start..).collect();
                result.extend(block);
                blocks_placed += 1;
                blocks_from[i] += 1;
            }
        }

        let l1s_used = blocks_from.iter().filter(|&&b| b > 0).count();
        if result.len() == total && l1s_used >= 2 {
            Some(result)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // InterGroupSameL1
    // -----------------------------------------------------------------------

    fn place_inter_group_same_l1(
        &mut self,
        total: usize,
        block_size: usize,
        used: &HashSet<Id>,
    ) -> Option<Vec<Id>> {
        if block_size == 0 || total % block_size != 0 {
            return None;
        }
        let num_blocks = total / block_size;

        let mut cell_names: Vec<String> = self.view.cells.keys().cloned().collect();
        cell_names.shuffle(&mut self.rng);

        let mut tagged_pools: Vec<(String, Vec<Id>)> = cell_names
            .iter()
            .flat_map(|cell| {
                let racks = match self.view.cells.get(cell) {
                    Some(r) => r,
                    None => return vec![],
                };
                racks
                    .values()
                    .flat_map(|l1s| l1s.values())
                    .map(|nodes| {
                        let free: Vec<Id> = nodes
                            .iter()
                            .filter(|id| !used.contains(*id))
                            .cloned()
                            .collect();
                        (cell.clone(), free)
                    })
                    .filter(|(_, pool)| pool.len() >= block_size)
                    .collect::<Vec<_>>()
            })
            .collect();

        let distinct_cells: HashSet<&str> = tagged_pools.iter().map(|(c, _)| c.as_str()).collect();
        if distinct_cells.len() < 2 {
            return None;
        }

        let total_slots: usize = tagged_pools.iter().map(|(_, p)| p.len() / block_size).sum();
        if total_slots < num_blocks {
            return None;
        }

        tagged_pools.shuffle(&mut self.rng);
        for (_, pool) in &mut tagged_pools {
            pool.shuffle(&mut self.rng);
        }
        tagged_pools.sort_by(|(_, a), (_, b)| b.len().cmp(&a.len()));

        let dominant_cell = tagged_pools[0].0.clone();
        let dominant_slots: usize = tagged_pools
            .iter()
            .filter(|(c, _)| c == &dominant_cell)
            .map(|(_, p)| p.len() / block_size)
            .sum();
        let max_from_dominant = if dominant_slots >= num_blocks {
            num_blocks - 1
        } else {
            dominant_slots
        };
        let mut used_from_dominant = 0usize;

        let mut result: Vec<Id> = Vec::with_capacity(total);
        let mut blocks_placed = 0usize;

        for (cell, pool) in &mut tagged_pools {
            if blocks_placed >= num_blocks {
                break;
            }
            let cap = pool.len() / block_size;
            let cell_limit = if cell == &dominant_cell {
                max_from_dominant.saturating_sub(used_from_dominant)
            } else {
                cap
            };
            let take = cell_limit.min(num_blocks - blocks_placed);
            for _ in 0..take {
                if pool.len() < block_size {
                    break;
                }
                let block: Vec<Id> = pool.drain(pool.len() - block_size..).collect();
                result.extend(block);
                blocks_placed += 1;
                if cell == &dominant_cell {
                    used_from_dominant += 1;
                }
            }
        }

        if result.len() == total && spans_multiple_cells(&result, &self.ir) {
            Some(result)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // Helper: pick n nodes from a cell spanning ≥2 L1 domains
    // -----------------------------------------------------------------------

    /// Round-robin across L1 pools within a cell until n nodes are collected,
    /// then verify the result actually spans ≥2 L1 domains.
    ///
    /// Pools are first shuffled (seed-dependent) then sorted descending by
    /// size so we drain the richest L1 first — this minimises the chance that
    /// a small pool runs dry partway through and breaks the ≥2-L1 invariant.
    fn pick_spanning_l1(&mut self, cell: &str, n: usize, used: &HashSet<Id>) -> Option<Vec<Id>> {
        let racks = self.view.cells.get(cell)?;

        let mut l1_pools: Vec<Vec<Id>> = racks
            .values()
            .flat_map(|l1s| l1s.iter())
            .map(|(_, nodes)| {
                nodes
                    .iter()
                    .filter(|id| !used.contains(*id))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .filter(|pool| !pool.is_empty())
            .collect();

        if l1_pools.len() < 2 {
            return None;
        }

        l1_pools.shuffle(&mut self.rng);
        // Sort descending by size so we don't exhaust a tiny pool too early.
        l1_pools.sort_by(|a, b| b.len().cmp(&a.len()));

        // Reserve capacity: cap the dominant L1's contribution to n-1 when it
        // alone could supply all n nodes, ensuring ≥2 L1s are represented.
        let dominant_cap = if l1_pools[0].len() >= n { n - 1 } else { l1_pools[0].len() };

        let mut result: Vec<Id> = Vec::with_capacity(n);

        // Drain from dominant L1 up to its cap.
        let take = dominant_cap.min(n);
        result.extend(l1_pools[0].drain(..take));

        // Round-robin remaining L1s for the rest.
        let mut pi = 1usize;
        while result.len() < n {
            let remaining = l1_pools.len() - 1;
            if remaining == 0 {
                break;
            }
            let idx = 1 + (pi % remaining);
            if let Some(id) = l1_pools[idx].pop() {
                result.push(id);
            } else {
                l1_pools.remove(idx);
                continue;
            }
            pi += 1;
        }

        let distinct_l1s = count_distinct_l1s(&result, cell, &self.ir);
        if result.len() == n && distinct_l1s >= 2 {
            Some(result)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Verification helpers
// ---------------------------------------------------------------------------

fn l1_of(node: &Id, ir: &TopologyIR) -> Option<Id> {
    ir.contains
        .iter()
        .find(|(_, children)| children.contains(node))
        .map(|(parent, _)| parent.clone())
}

fn cell_of(node: &Id, ir: &TopologyIR) -> Option<String> {
    let l1 = l1_of(node, ir)?;
    ir.entities.get(&l1)?.meta.get("cell").cloned()
}

fn count_distinct_l1s(nodes: &[Id], _cell: &str, ir: &TopologyIR) -> usize {
    nodes
        .iter()
        .filter_map(|id| l1_of(id, ir))
        .collect::<HashSet<_>>()
        .len()
}

fn spans_multiple_cells(nodes: &[Id], ir: &TopologyIR) -> bool {
    let cells: HashSet<String> = nodes.iter().filter_map(|id| cell_of(id, ir)).collect();
    cells.len() >= 2
}