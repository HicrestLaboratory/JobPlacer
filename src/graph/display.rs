use petgraph::graph::Graph as PetGraph;
use petgraph::visit::EdgeRef;
use crate::ir::topology_ir::TopologyIR;
use crate::ir::id::Id;
use crate::ir::entity::EntityKind;
use std::collections::{HashSet, HashMap};
use std::io::Write;

/// Maps job name -> set of compute node Id strings.
pub type Allocations = HashMap<String, HashSet<String>>;

// ---------------------------------------------------------------------------
// Color / pattern palette  (Cartesian product: 10 colors × 5 styles = 50)
// ---------------------------------------------------------------------------

const BASE_COLORS: &[&str] = &[
    "#E63946", "#2196F3", "#4CAF50", "#FF9800", "#9C27B0",
    "#00BCD4", "#FFEB3B", "#795548", "#607D8B", "#E91E63",
];

const PATTERNS: &[&str] = &[
    "filled",
    "filled,dashed",
    "filled,dotted",
    "filled,bold",
    "diagonals,filled",
];

fn allocation_style(index: usize) -> (&'static str, &'static str) {
    let color   = BASE_COLORS[index % BASE_COLORS.len()];
    let pattern = PATTERNS[(index / BASE_COLORS.len()) % PATTERNS.len()];
    (color, pattern)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn get_label(id: &Id, ir: &TopologyIR) -> String {
    match ir.entities.get(id) {
        Some(entity) => match &entity.kind {
            EntityKind::Compute              => id.0.clone(),
            EntityKind::Group                => format!("Group {}", id.0),
            EntityKind::Switch { level }     => format!("{} (L{})", id.0, level.unwrap_or(0)),
        },
        None => id.0.clone(),
    }
}

/// Returns the entity level:
///   L0 = compute / unknown  (leaves)
///   L1 = first-tier switch  (connects directly to compute)
///   L2 = second-tier switch (connects L1 switches / global)
fn entity_level(id: &Id, ir: &TopologyIR) -> u32 {
    match ir.entities.get(id) {
        Some(e) => match &e.kind {
            EntityKind::Compute             => 0,
            EntityKind::Group               => 0,
            EntityKind::Switch { level }    => level.unwrap_or(1) as u32,
        },
        None => 0,
    }
}

fn node_base_style(id: &Id, ir: &TopologyIR) -> (&'static str, &'static str, &'static str) {
    // returns (fillcolor, shape, style)
    match ir.entities.get(id) {
        Some(e) => match &e.kind {
            EntityKind::Compute => ("#AED6F1", "box",     "filled,rounded"),
            EntityKind::Switch { level } => match level.unwrap_or(1) {
                1 => ("#A9DFBF", "diamond", "filled"),
                _ => ("#F9E79F", "hexagon", "filled"),   // L2+
            },
            EntityKind::Group   => ("#D7DBDD", "ellipse", "filled"),
        },
        None => ("#AED6F1", "box", "filled,rounded"),
    }
}

fn write_node(
    file: &mut std::fs::File,
    id: &Id,
    ir: &TopologyIR,
    node_alloc: &HashMap<String, (String, usize)>,
    indent: &str,
) {
    let label = get_label(id, ir);
    let (base_color, shape, base_style) = node_base_style(id, ir);

    let attrs = if let Some((job, idx)) = node_alloc.get(&id.0) {
        let (color, style) = allocation_style(*idx);
        format!(
            "shape={shape} style=\"{style}\" fillcolor=\"{color}\" \
             label=\"{label}\" tooltip=\"{job}\" \
             margin=0.05 width=0 height=0 fontsize=9",
        )
    } else {
        format!(
            "shape={shape} style=\"{base_style}\" fillcolor=\"{base_color}\" \
             label=\"{label}\" \
             margin=0.05 width=0 height=0 fontsize=9",
        )
    };

    writeln!(file, "{indent}\"{}\" [{}];", label, attrs).unwrap();
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub fn display_graph<Ty>(
    graph: &PetGraph<Id, f32, Ty>,
    ir: &TopologyIR,
    output_file: &str,
    allocations: Option<&Allocations>,
) where
    Ty: petgraph::EdgeType,
{
    // ------------------------------------------------------------------
    // Build allocation lookup: node_id_str -> (job_name, palette_index)
    // ------------------------------------------------------------------
    let mut node_alloc: HashMap<String, (String, usize)> = HashMap::new();
    if let Some(allocs) = allocations {
        let mut jobs: Vec<&String> = allocs.keys().collect();
        jobs.sort();
        for (idx, job) in jobs.iter().enumerate() {
            for node_id in &allocs[*job] {
                node_alloc.insert(node_id.clone(), ((*job).clone(), idx));
            }
        }
    }

    // ------------------------------------------------------------------
    // Classify nodes by level
    //   L0 → compute leaves
    //   L1 → first-tier switches  (level == 1)
    //   L2 → second-tier switches (level >= 2)
    // ------------------------------------------------------------------
    let mut l1_nodes: Vec<petgraph::graph::NodeIndex> = Vec::new();
    let mut l2_nodes: Vec<petgraph::graph::NodeIndex> = Vec::new();
    let mut l0_nodes: Vec<petgraph::graph::NodeIndex> = Vec::new();

    for ni in graph.node_indices() {
        match entity_level(&graph[ni], ir) {
            1      => l1_nodes.push(ni),
            l if l >= 2 => l2_nodes.push(ni),
            _      => l0_nodes.push(ni),
        }
    }

    // ------------------------------------------------------------------
    // Build per-L1-switch clusters via graph adjacency.
    // For every L1 switch, collect its directly connected L0 nodes.
    // A compute node is assigned to the first L1 switch it appears under
    // (sorted by id) to avoid duplicates.
    // ------------------------------------------------------------------
    let mut l1_children: HashMap<petgraph::graph::NodeIndex, Vec<petgraph::graph::NodeIndex>> =
        HashMap::new();
    let mut assigned_l0: HashSet<petgraph::graph::NodeIndex> = HashSet::new();

    let mut l1_sorted = l1_nodes.clone();
    l1_sorted.sort_by_key(|&ni| &graph[ni].0);

    for &l1 in &l1_sorted {
        let mut children: Vec<_> = graph
            .edges(l1)
            .filter_map(|e| {
                let nb = if e.source() == l1 { e.target() } else { e.source() };
                if entity_level(&graph[nb], ir) == 0 && !assigned_l0.contains(&nb) {
                    Some(nb)
                } else {
                    None
                }
            })
            .collect();
        children.sort_by_key(|&ni| &graph[ni].0);
        for &c in &children {
            assigned_l0.insert(c);
        }
        l1_children.insert(l1, children);
    }

    let ungrouped_l0: Vec<_> = l0_nodes
        .iter()
        .filter(|&&ni| !assigned_l0.contains(&ni))
        .copied()
        .collect();

    // ------------------------------------------------------------------
    // Write DOT file
    // ------------------------------------------------------------------
    let dot_file = "topology.dot";
    let mut file = std::fs::File::create(dot_file).expect("Failed to create DOT file");

    writeln!(file, "graph G {{").unwrap();
    writeln!(file, "  rankdir=TB;").unwrap();
    writeln!(file, "  splines=polyline;").unwrap();
    writeln!(file, "  nodesep=0.25;").unwrap();
    writeln!(file, "  ranksep=0.8;").unwrap();
    writeln!(file, "  fontname=\"Helvetica\"; fontsize=10;").unwrap();
    writeln!(file, "  edge [dir=none, penwidth=1.2, color=\"#555555\"];").unwrap();
    writeln!(file, "  node [fontname=\"Helvetica\"];").unwrap();
    writeln!(file).unwrap();

    // ---- L2 switches (top tier, rank=min) --------------------------------
    if !l2_nodes.is_empty() {
        writeln!(file, "  subgraph cluster_l2 {{").unwrap();
        writeln!(file, "    label=\"L2 Switches\"; style=dashed; color=\"#888888\";").unwrap();
        writeln!(file, "    rank=min;").unwrap();
        let mut l2_sorted = l2_nodes.clone();
        l2_sorted.sort_by_key(|&ni| &graph[ni].0);
        for &ni in &l2_sorted {
            write_node(&mut file, &graph[ni], ir, &node_alloc, "    ");
        }
        writeln!(file, "  }}").unwrap();
        writeln!(file).unwrap();
    }

    // ---- One cluster per L1 switch ---------------------------------------
    // Each cluster contains the L1 switch node + all its L0 children.
    for (cl_idx, &l1) in l1_sorted.iter().enumerate() {
        let l1_id    = &graph[l1];
        let l1_label = get_label(l1_id, ir);
        writeln!(file, "  subgraph cluster_l1_{cl_idx} {{").unwrap();
        writeln!(
            file,
            "    label=\"{l1_label}\"; style=filled; fillcolor=\"#F4F6F7\"; \
             color=\"#2C3E50\"; penwidth=1.5; margin=6;",
        ).unwrap();

        write_node(&mut file, l1_id, ir, &node_alloc, "    ");

        for &c in &l1_children[&l1] {
            write_node(&mut file, &graph[c], ir, &node_alloc, "    ");
        }
        writeln!(file, "  }}").unwrap();
        writeln!(file).unwrap();
    }

    // ---- Compute nodes not under any L1 switch ---------------------------
    if !ungrouped_l0.is_empty() {
        writeln!(file, "  subgraph cluster_ungrouped {{").unwrap();
        writeln!(
            file,
            "    label=\"Compute (ungrouped)\"; style=dashed; color=\"#AAB7B8\";",
        ).unwrap();
        let mut ug = ungrouped_l0.clone();
        ug.sort_by_key(|&ni| &graph[ni].0);
        for &ni in &ug {
            write_node(&mut file, &graph[ni], ir, &node_alloc, "    ");
        }
        writeln!(file, "  }}").unwrap();
        writeln!(file).unwrap();
    }

    // ---- Edges -----------------------------------------------------------
    let mut drawn_edges: HashSet<(String, String)> = HashSet::new();
    for edge in graph.edge_references() {
        let a  = &graph[edge.source()];
        let b  = &graph[edge.target()];
        let la = get_label(a, ir);
        let lb = get_label(b, ir);
        let key = if la < lb { (la.clone(), lb.clone()) } else { (lb.clone(), la.clone()) };
        if drawn_edges.insert(key) {
            writeln!(file, "  \"{la}\" -- \"{lb}\";").unwrap();
        }
    }

    // ---- Allocation legend -----------------------------------------------
    if let Some(allocs) = allocations {
        if !allocs.is_empty() {
            writeln!(file).unwrap();
            writeln!(file, "  subgraph cluster_legend {{").unwrap();
            writeln!(
                file,
                "    label=\"Allocations\"; style=filled; fillcolor=\"#FDFEFE\"; \
                 color=\"#2C3E50\"; rank=sink;",
            ).unwrap();
            let mut jobs: Vec<&String> = allocs.keys().collect();
            jobs.sort();
            for (idx, job) in jobs.iter().enumerate() {
                let (color, style) = allocation_style(idx);
                writeln!(
                    file,
                    "    \"legend_{idx}\" [label=\"{job}\" shape=box \
                     style=\"{style}\" fillcolor=\"{color}\" \
                     margin=0.05 width=0 height=0 fontsize=9];",
                ).unwrap();
            }
            writeln!(file, "  }}").unwrap();
        }
    }

    writeln!(file, "}}").unwrap();

    // ------------------------------------------------------------------
    // Invoke Graphviz
    // ------------------------------------------------------------------
    let format = if output_file.ends_with(".png") {
        "png"
    } else if output_file.ends_with(".svg") {
        "svg"
    } else {
        panic!("Output must end with .png or .svg");
    };

    let status = std::process::Command::new("dot")
        .arg(format!("-T{format}"))
        .arg(dot_file)
        .arg("-o")
        .arg(output_file)
        .status()
        .expect("Failed to run Graphviz (please install it on your system)");

    if status.success() {
        println!("Graph image generated at {output_file}");
    } else {
        eprintln!("Graphviz failed");
    }
}