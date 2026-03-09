// placement.rs

use std::collections::{BTreeMap, HashSet};

use rand::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ir::entity::EntityKind;
use crate::ir::id::Id;
use crate::ir::topology_ir::TopologyIR;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlacementClass {
    IntraL1,
    IntraGroup,
    InterGroup,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobRequest {
    pub strategy: String,
    pub nodes: usize,
    pub placement_class: PlacementClass,
}

/// A successfully placed job: the exact compute node IDs assigned.
#[derive(Debug, Clone, Serialize)]
pub struct JobPlacement {
    pub nodes: Vec<String>,
    pub placement_class: String,
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
    Infeasible {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Topology views
// ---------------------------------------------------------------------------

/// Flat view of the topology needed by the placer.
struct TopoView {
    /// cell_id → rack_id → l1_switch_id → [compute_node_id]
    cells: BTreeMap<String, BTreeMap<String, BTreeMap<Id, Vec<Id>>>>,
}

impl TopoView {
    fn build(ir: &TopologyIR) -> Self {
        let mut cells: BTreeMap<String, BTreeMap<String, BTreeMap<Id, Vec<Id>>>> = BTreeMap::new();

        // Collect all L1 switches grouped by cell + rack
        for entity in ir.entities.values() {
            if !matches!(entity.kind, EntityKind::Switch { level: Some(0) }) {
                continue;
            }
            let cell = entity.meta.get("cell").cloned().unwrap_or_else(|| "ungrouped".into());
            let rack = entity.meta.get("rack").cloned().unwrap_or_else(|| "?".into());

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

    /// All compute nodes under a single L1 switch.
    fn nodes_in_l1<'a>(&'a self, cell: &str, rack: &str, l1: &Id) -> Vec<&'a Id> {
        self.cells
            .get(cell)
            .and_then(|r| r.get(rack))
            .and_then(|l| l.get(l1))
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// All compute nodes in a cell (across all racks and L1 switches).
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
}

// ---------------------------------------------------------------------------
// Placement engine
// ---------------------------------------------------------------------------

pub struct Placer<'a> {
    ir:   &'a TopologyIR,
    view: TopoView,
    rng:  SmallRng,
}

impl<'a> Placer<'a> {
    pub fn new(ir: &'a TopologyIR, seed: u64) -> Self {
        Self {
            ir,
            view: TopoView::build(ir),
            rng: SmallRng::seed_from_u64(seed),
        }
    }

    /// Attempt to place all jobs simultaneously with non-overlapping nodes.
    pub fn place(
        &mut self,
        jobs: &BTreeMap<String, JobRequest>,
    ) -> PlacementResult {
        // Sort jobs by descending node count so larger jobs are placed first
        // (greedy: harder constraints first reduces backtracking).
        let mut job_order: Vec<(&String, &JobRequest)> = jobs.iter().collect();
        job_order.sort_by(|a, b| b.1.nodes.cmp(&a.1.nodes));

        let mut used: HashSet<Id> = HashSet::new();
        let mut placements: BTreeMap<String, JobPlacement> = BTreeMap::new();

        for (job_name, req) in &job_order {
            match self.place_one(req, &used) {
                Some(assigned) => {
                    for id in &assigned {
                        used.insert(id.clone());
                    }
                    placements.insert(
                        (*job_name).clone(),
                        JobPlacement {
                            nodes: assigned.into_iter().map(|id| id.0).collect(),
                            placement_class: format!("{:?}", req.placement_class),
                        },
                    );
                }
                None => {
                    return PlacementResult::Infeasible {
                        reason: format!(
                            "cannot place job '{}': need {} nodes with class {:?}, \
                             not enough free nodes satisfy the constraint",
                            job_name, req.nodes, req.placement_class
                        ),
                    };
                }
            }
        }

        PlacementResult::Ok { placements }
    }

    // -----------------------------------------------------------------------
    // Single-job placement
    // -----------------------------------------------------------------------

    fn place_one(&mut self, req: &JobRequest, used: &HashSet<Id>) -> Option<Vec<Id>> {
        match req.placement_class {
            PlacementClass::IntraL1    => self.place_intra_l1(req.nodes, used),
            PlacementClass::IntraGroup => self.place_intra_group(req.nodes, used),
            PlacementClass::InterGroup => self.place_inter_group(req.nodes, used),
        }
    }

    /// IntraL1: all nodes must come from a single L1-switch domain.
    fn place_intra_l1(&mut self, n: usize, used: &HashSet<Id>) -> Option<Vec<Id>> {
        // Collect all (cell, rack, l1_id) triples that have enough free nodes
        let mut candidates: Vec<(String, String, Id)> = self
            .view
            .cells
            .iter()
            .flat_map(|(cell, racks)| {
                racks.iter().flat_map(move |(rack, l1s)| {
                    l1s.keys().map(move |l1| (cell.clone(), rack.clone(), l1.clone()))
                })
            })
            .filter(|(cell, rack, l1)| {
                let free = self
                    .view
                    .nodes_in_l1(cell, rack, l1)
                    .into_iter()
                    .filter(|id| !used.contains(*id))
                    .count();
                free >= n
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

    /// IntraGroup: all nodes within one cell, but spanning >1 L1 domain.
    ///
    /// We first try to satisfy the constraint strictly (nodes span ≥2 L1s).
    /// If n=1 we relax to any single node inside the cell.
    fn place_intra_group(&mut self, n: usize, used: &HashSet<Id>) -> Option<Vec<Id>> {
        let mut cell_names: Vec<String> = self.view.cells.keys().cloned().collect();
        cell_names.shuffle(&mut self.rng);

        for cell in &cell_names {
            let free: Vec<Id> = self
                .view
                .nodes_in_cell(cell)
                .into_iter()
                .filter(|id| !used.contains(*id))
                .cloned()
                .collect();

            if free.len() < n { continue; }

            // For n>1 ensure we span at least 2 L1 domains (intra-group means
            // traffic crosses the group fabric, not just one L1 switch).
            let mut selected = self.pick_spanning_l1(cell, n, used);
            if selected.is_none() && n == 1 {
                // Single-node job: any node in the cell satisfies intra-group
                let mut pool = free;
                pool.shuffle(&mut self.rng);
                selected = pool.into_iter().next().map(|id| vec![id]);
            }
            if selected.is_some() { return selected; }
        }
        None
    }

    /// InterGroup: nodes must span ≥2 distinct cells.
    fn place_inter_group(&mut self, n: usize, used: &HashSet<Id>) -> Option<Vec<Id>> {
        let mut cell_names: Vec<String> = self.view.cells.keys().cloned().collect();
        cell_names.shuffle(&mut self.rng);

        // Collect free nodes per cell
        let mut per_cell: Vec<(String, Vec<Id>)> = cell_names
            .into_iter()
            .map(|cell| {
                let free: Vec<Id> = self
                    .view
                    .nodes_in_cell(&cell)
                    .into_iter()
                    .filter(|id| !used.contains(*id))
                    .cloned()
                    .collect();
                (cell, free)
            })
            .filter(|(_, free)| !free.is_empty())
            .collect();

        let total_free: usize = per_cell.iter().map(|(_, v)| v.len()).sum();
        if total_free < n || per_cell.len() < 2 { return None; }

        // Round-robin across cells to ensure we span ≥2
        let mut result: Vec<Id> = Vec::with_capacity(n);
        let mut ci = 0usize;
        while result.len() < n {
            let cell_len = per_cell.len();
            let (_, pool) = &mut per_cell[ci % cell_len];
            pool.shuffle(&mut self.rng);
            if let Some(id) = pool.pop() {
                result.push(id);
            }
            ci += 1;
            if ci > per_cell.len() * n { break; } // safety valve
        }

        if result.len() == n && spans_multiple_cells(&result, self.ir) {
            Some(result)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // Helper: pick n nodes from a cell spanning ≥2 L1 domains
    // -----------------------------------------------------------------------
    fn pick_spanning_l1(&mut self, cell: &str, n: usize, used: &HashSet<Id>) -> Option<Vec<Id>> {
        let racks = self.view.cells.get(cell)?;

        // Build per-L1 free node lists
        let mut l1_pools: Vec<Vec<Id>> = racks
            .values()
            .flat_map(|l1s| l1s.iter())
            .map(|(_, nodes)| {
                nodes.iter().filter(|id| !used.contains(*id)).cloned().collect::<Vec<_>>()
            })
            .filter(|pool| !pool.is_empty())
            .collect();

        if l1_pools.len() < 2 { return None; }
        l1_pools.shuffle(&mut self.rng);

        // Round-robin across L1 pools to ensure ≥2 are represented
        let mut result: Vec<Id> = Vec::with_capacity(n);
        let mut pi = 0usize;
        while result.len() < n {
            let l1_pools_len = l1_pools.len();
            let pool = &mut l1_pools[pi % l1_pools_len];
            pool.shuffle(&mut self.rng);
            if let Some(id) = pool.pop() {
                result.push(id);
            }
            pi += 1;
            if pi > l1_pools.len() * n { break; }
        }

        // Verify we actually span ≥2 L1 domains
        let distinct_l1s = count_distinct_l1s(&result, cell, self.ir);
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
    let cells: HashSet<String> = nodes
        .iter()
        .filter_map(|id| cell_of(id, ir))
        .collect();
    cells.len() >= 2
}