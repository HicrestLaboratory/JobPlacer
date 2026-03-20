//! DOT/Graphviz renderer — cluster-based Dragonfly layout.
//!
//! # Layout hierarchy (maps directly to DOT subgraph clusters)
//! ```
//! graph G
//!  └─ cluster_cell_N        (one per cell, labelled with cell id)
//!      └─ cluster_rack_N_R  (one per rack inside the cell)
//!          └─ cluster_l1_N_R_L  (one per L1 switch: the switch + its compute nodes)
//!              ├─ L1 switch node  (diamond)
//!              └─ compute nodes   (rounded rectangles)
//! ```
//!
//! `dot` enforces containment: nodes declared inside a cluster are always
//! drawn inside its bounding box.  No manual coordinate math needed.
//!
//! Inter-cell dragonfly links are omitted; the count is annotated on each
//! cell cluster label.  L2 switches are optionally rendered inside the rack
//! cluster, controlled by [`DisplayOptions`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;

use log::info;

use crate::ir::topology_ir::TopologyIR;
use crate::ir::Id;
use crate::ir::{Entity, EntityKind};

pub type Allocations = HashMap<String, HashSet<String>>;

// ---------------------------------------------------------------------------
// Display options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DisplayOptions {
    /// Render L2 switches inside each rack cluster. Default: false.
    pub show_l2_switches: bool,
    /// Render edges (L1↔compute, and optionally L1↔L2). Default: false.
    pub show_edges: bool,
    /// Render groups inter_cell_links_count. Default: false.
    pub show_inter_cell_links_count: bool,
}

impl Default for DisplayOptions {
    fn default() -> Self {
        Self {
            show_l2_switches: false,
            show_edges: false,
            show_inter_cell_links_count: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Colour palette
// ---------------------------------------------------------------------------

const BASE_COLORS: &[&str] = &[
    "#E63946", "#2196F3", "#4CAF50", "#FF9800", "#9C27B0", "#00BCD4", "#FFEB3B", "#795548",
    "#607D8B", "#E91E63",
];
const PATTERNS: &[&str] = &[
    "filled",
    "filled,dashed",
    "filled,dotted",
    "filled,bold",
    "diagonals,filled",
];
fn alloc_style(i: usize) -> (&'static str, &'static str) {
    (
        BASE_COLORS[i % BASE_COLORS.len()],
        PATTERNS[(i / BASE_COLORS.len()) % PATTERNS.len()],
    )
}

// ---------------------------------------------------------------------------
// IR helpers
// ---------------------------------------------------------------------------

/// All L1 switches grouped by cell → rack, both levels sorted for stability.
fn collect_cell_rack_l1(ir: &TopologyIR) -> BTreeMap<String, BTreeMap<String, Vec<Id>>> {
    let mut out: BTreeMap<String, BTreeMap<String, Vec<Id>>> = BTreeMap::new();
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
            .or_else(|| entity.meta.get("cabinet").cloned())
            .unwrap_or_else(|| "?".into());
        out.entry(cell)
            .or_default()
            .entry(rack)
            .or_default()
            .push(entity.id.clone());
    }
    for racks in out.values_mut() {
        for switches in racks.values_mut() {
            switches.sort_by(|a, b| a.0.cmp(&b.0));
        }
    }
    out
}

fn compute_children(l1_id: &Id, ir: &TopologyIR) -> Vec<Id> {
    let mut nodes: Vec<Id> = ir
        .contains
        .get(l1_id)
        .map(|ch| {
            ch.iter()
                .filter(|id| {
                    matches!(
                        ir.entities.get(id),
                        Some(Entity {
                            kind: EntityKind::Compute,
                            ..
                        })
                    )
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    nodes
}

fn l2_in_rack(cell: &str, rack: &str, ir: &TopologyIR) -> Vec<Id> {
    let mut out: Vec<Id> = ir
        .entities
        .values()
        .filter(|e| {
            matches!(e.kind, EntityKind::Switch { level: Some(1) })
                && e.meta.get("cell").map(|s| s.as_str()) == Some(cell)
                && e.meta.get("rack").map(|s| s.as_str()) == Some(rack)
        })
        .map(|e| e.id.clone())
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn inter_cell_link_count(cell: &str, ir: &TopologyIR) -> usize {
    let cell_ids: HashSet<&Id> = ir
        .entities
        .values()
        .filter(|e| {
            matches!(e.kind, EntityKind::Switch { .. })
                && e.meta.get("cell").map(|s| s.as_str()) == Some(cell)
        })
        .map(|e| &e.id)
        .collect();

    ir.links
        .iter()
        .filter(|lnk| {
            lnk.weight >= 2.0 && (cell_ids.contains(&lnk.from) || cell_ids.contains(&lnk.to))
        })
        .count()
}

// ---------------------------------------------------------------------------
// DOT node emitters
// ---------------------------------------------------------------------------

fn emit_l1_switch(out: &mut impl Write, id: &Id, alloc: &HashMap<String, (String, usize)>) {
    let (fill, style, tooltip) = if let Some((job, idx)) = alloc.get(&id.0) {
        let (c, s) = alloc_style(*idx);
        (c.to_string(), s.to_string(), format!(" tooltip=\"{job}\""))
    } else {
        ("#A9DFBF".to_string(), "filled".to_string(), String::new())
    };
    writeln!(
        out,
        "    \"{id}\" [shape=ellipse style=\"{style}\" fillcolor=\"{fill}\"{tooltip} \
         label=\"{id}\" fontsize=7 width=0.72 height=0.25 fixedsize=true];",
        id = id.0,
    )
    .unwrap();
}

fn emit_l2_switch(out: &mut impl Write, id: &Id, alloc: &HashMap<String, (String, usize)>) {
    let (fill, style, tooltip) = if let Some((job, idx)) = alloc.get(&id.0) {
        let (c, s) = alloc_style(*idx);
        (c.to_string(), s.to_string(), format!(" tooltip=\"{job}\""))
    } else {
        ("#F0B27A".to_string(), "filled".to_string(), String::new())
    };
    writeln!(
        out,
        "    \"{id}\" [shape=hexagon style=\"{style}\" fillcolor=\"{fill}\"{tooltip} \
         label=\"{id}\" fontsize=7 width=0.55 height=0.38 fixedsize=true];",
        id = id.0,
    )
    .unwrap();
}

fn emit_compute_node(out: &mut impl Write, id: &Id, alloc: &HashMap<String, (String, usize)>) {
    let (fill, style, tooltip) = if let Some((job, idx)) = alloc.get(&id.0) {
        let (c, s) = alloc_style(*idx);
        (c.to_string(), s.to_string(), format!(" tooltip=\"{job}\""))
    } else {
        (
            "#AED6F1".to_string(),
            "filled,rounded".to_string(),
            String::new(),
        )
    };
    writeln!(
        out,
        "    \"{id}\" [shape=box style=\"{style}\" fillcolor=\"{fill}\"{tooltip} \
         label=\"{id}\" fontsize=6 width=0.72 height=0.22 fixedsize=true];",
        id = id.0,
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub fn display_graph(
    ir: &TopologyIR,
    output_file: &str,
    allocations: Option<&Allocations>,
    opts: &DisplayOptions,
) {
    let mut alloc: HashMap<String, (String, usize)> = HashMap::new();
    if let Some(a) = allocations {
        let mut jobs: Vec<&String> = a.keys().collect();
        jobs.sort();
        for (i, job) in jobs.iter().enumerate() {
            for nid in &a[*job] {
                alloc.insert(nid.clone(), ((*job).clone(), i));
            }
        }
    }

    let cell_rack_l1 = collect_cell_rack_l1(ir);
    let dot_path = "topology.dot";
    let mut f = std::fs::File::create(dot_path).expect("cannot create topology.dot");

    writeln!(f, "digraph G {{").unwrap();
    writeln!(f, "  layout=dot;").unwrap();
    writeln!(f, "  rankdir=TB;").unwrap(); // can also use RL (right-left)
    writeln!(f, "  newrank=true;").unwrap();
    writeln!(f, "  compound=true;").unwrap();
    writeln!(f, "  clusterrank=local;").unwrap();
    writeln!(f, "  ranksep=0.25;").unwrap();
    writeln!(f, "  nodesep=0.35;").unwrap();
    writeln!(f, "  splines=false;").unwrap(); // this curves edges
    writeln!(f, "  node [fontname=\"Helvetica\"];").unwrap();
    writeln!(f, "  edge [dir=none penwidth=0.6 color=\"#888888\"];").unwrap();
    writeln!(f, "  graph [fontname=\"Helvetica\" labeljust=l];").unwrap();
    writeln!(f).unwrap();

    let mut edges: Vec<(String, String)> = Vec::new();

    // NEW: anchors for aligning cells horizontally
    let mut cell_anchors: Vec<String> = Vec::new();

    for (ci, (cell_name, racks)) in cell_rack_l1.iter().enumerate() {
        let ice = inter_cell_link_count(cell_name, ir);
        let ice_str = if ice > 0 && opts.show_inter_cell_links_count {
            format!(" (↔ {ice} inter-cell links)")
        } else {
            String::new()
        };

        writeln!(f, "  subgraph cluster_cell_{ci} {{").unwrap();
        writeln!(f, "    label=\"{cell_name}{ice_str}\";").unwrap();
        writeln!(f, "    style=\"filled,rounded\";").unwrap();
        writeln!(f, "    fillcolor=\"#EBF5FB\";").unwrap();
        writeln!(f, "    color=\"#1A5276\";").unwrap();
        writeln!(f, "    penwidth=2.5;").unwrap();
        writeln!(f, "    fontsize=11;").unwrap();

        let anchor = format!("__cell_anchor_{ci}");
        cell_anchors.push(anchor.clone());
        writeln!(
            f,
            "    \"{anchor}\" [shape=point width=0 height=0 label=\"\" style=invis];"
        )
        .unwrap();

        writeln!(f).unwrap();

        for (ri, (rack_name, l1_ids)) in racks.iter().enumerate() {
            writeln!(f, "    subgraph cluster_rack_{ci}_{ri} {{").unwrap();
            writeln!(f, "      label=\"rack {rack_name}\";").unwrap();
            writeln!(f, "      style=\"filled,rounded\";").unwrap();
            writeln!(f, "      fillcolor=\"#F2F3F4\";").unwrap();
            writeln!(f, "      color=\"#2C3E50\";").unwrap();
            writeln!(f, "      penwidth=1.5;").unwrap();
            writeln!(f, "      fontsize=9;").unwrap();
            writeln!(f).unwrap();

            if opts.show_l2_switches {
                let l2_ids = l2_in_rack(cell_name, rack_name, ir);
                for l2_id in &l2_ids {
                    emit_l2_switch(&mut f, l2_id, &alloc);

                    if opts.show_edges {
                        for l1_id in l1_ids {
                            let l1_pair = ir
                                .entities
                                .get(l1_id)
                                .and_then(|e| e.meta.get("pair_index"));
                            let l2_pair = ir
                                .entities
                                .get(l2_id)
                                .and_then(|e| e.meta.get("pair_index"));
                            if l1_pair.is_some() && l1_pair == l2_pair {
                                edges.push((l1_id.0.clone(), l2_id.0.clone()));
                            }
                        }
                    }
                }
                writeln!(f).unwrap();
            }

            // L1-group sub-clusters
            for (li, l1_id) in l1_ids.iter().enumerate() {
                let compute = compute_children(l1_id, ir);

                writeln!(f, "      subgraph cluster_l1_{ci}_{ri}_{li} {{").unwrap();
                writeln!(f, "        label=\"\";").unwrap();
                writeln!(f, "        style=\"filled,rounded\";").unwrap();
                writeln!(f, "        fillcolor=\"#FDFEFE\";").unwrap();
                writeln!(f, "        color=\"#AAB7B8\";").unwrap();
                writeln!(f, "        penwidth=0.8;").unwrap();
                writeln!(f).unwrap();

                emit_l1_switch(&mut f, l1_id, &alloc);

                for node_id in &compute {
                    emit_compute_node(&mut f, node_id, &alloc);

                    if opts.show_edges {
                        edges.push((l1_id.0.clone(), node_id.0.clone()));
                    }
                }

                // Force vertical ordering using invisible edges
                if !compute.is_empty() {
                    writeln!(
                        f,
                        "        \"{}\" -> \"{}\" [style=invis weight=2 minlen=1];",
                        l1_id.0, compute[0].0
                    )
                    .unwrap();

                    for pair in compute.windows(2) {
                        writeln!(
                            f,
                            "        \"{}\" -> \"{}\" [style=invis weight=2 minlen=1];",
                            pair[0].0, pair[1].0
                        )
                        .unwrap();
                    }
                }

                writeln!(f, "      }}").unwrap();
            }

            writeln!(f, "    }}").unwrap();
            writeln!(f).unwrap();
        }

        writeln!(f, "  }}").unwrap();
        writeln!(f).unwrap();
    }

    // ===================================================================
    // Force all cell clusters onto the same horizontal rank
    // ===================================================================
    if cell_anchors.len() > 1 {
        writeln!(f).unwrap();
        writeln!(f, "  {{ rank=same;").unwrap();
        for a in &cell_anchors {
            writeln!(f, "    \"{a}\";").unwrap();
        }
        writeln!(f, "  }}").unwrap();

        for pair in cell_anchors.windows(2) {
            writeln!(f, "  \"{}\" -> \"{}\" [style=invis];", pair[0], pair[1]).unwrap();
        }
    }

    if opts.show_edges {
        let mut drawn: HashSet<(String, String)> = HashSet::new();
        for (a, b) in &edges {
            let key = if a <= b {
                (a.clone(), b.clone())
            } else {
                (b.clone(), a.clone())
            };
            if drawn.insert(key) {
                writeln!(f, "  \"{a}\" -> \"{b}\";").unwrap();
            }
        }
    }

    if let Some(a) = allocations {
        if !a.is_empty() {
            writeln!(f).unwrap();
            writeln!(f, "  subgraph cluster_legend {{").unwrap();
            writeln!(
                f,
                "    label=\"Legend\"; style=filled; fillcolor=white; fontsize=10;"
            )
            .unwrap();

            let mut jobs: Vec<&String> = a.keys().collect();
            jobs.sort();

            for (i, job) in jobs.iter().enumerate() {
                let (color, style) = alloc_style(i);
                writeln!(
                    f,
                    "    \"__legend_{i}\" [shape=box style=\"{style}\" fillcolor=\"{color}\" \
                     label=\"{job}\" fontsize=8 width=2.4 height=0.28 fixedsize=true];",
                )
                .unwrap();

                if i > 0 {
                    writeln!(
                        f,
                        "    \"__legend_{}\" -> \"__legend_{}\" [style=invis];",
                        i - 1,
                        i
                    )
                    .unwrap();
                }
            }

            writeln!(f, "  }}").unwrap();
        }
    }

    writeln!(f, "}}").unwrap();
    drop(f);

    let fmt = if output_file.ends_with(".svg") {
        "svg"
    } else if output_file.ends_with(".png") {
        "png"
    } else {
        panic!("output must end in .svg or .png")
    };

    let ok = std::process::Command::new("dot")
        .args([format!("-T{fmt}").as_str(), dot_path, "-o", output_file])
        .status()
        .expect("failed to run `dot`")
        .success();

    if ok {
        info!("Graph written to {output_file}");
    } else {
        eprintln!("`dot` failed — DOT source preserved at {dot_path}");
    }
}
