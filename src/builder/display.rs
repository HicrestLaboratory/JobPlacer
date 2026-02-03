use crate::builder::graph::Graph;
use crate::ir::topology_ir::TopologyIR;
use crate::ir::entity::EntityKind;
use crate::ir::id::Id;
use std::process::Command;
use std::fs::File;
use std::io::Write;
use std::collections::HashSet;

/// Generate a clean tree-like graph image from a Graph and IR
pub fn generate_graph_image(graph: &Graph, ir: &TopologyIR, output_file: &str) {
    let dot_file = "topology.dot";
    let mut file = File::create(dot_file).expect("Failed to create DOT file");

    writeln!(file, "graph G {{").unwrap();
    writeln!(file, "  rankdir=TB;").unwrap(); // top-to-bottom tree layout
    writeln!(file, "  node [shape=box, style=filled, color=lightblue, fontname=\"Arial\"];\n").unwrap();
    writeln!(file, "  edge [dir=none, penwidth=2, color=black];\n").unwrap(); // bidirectional lines

    // Helper to get node label
    let get_label = |id: &Id| -> String {
        match ir.entities.get(id) {
            Some(entity) => match &entity.kind {
                EntityKind::Compute => id.0.clone(),
                EntityKind::Group => format!("Group {}", id.0),
                EntityKind::Switch { level } => format!("{} (L{})", id.0, level.unwrap_or(0)),
            },
            None => id.0.clone(),
        }
    };

    // Write all nodes
    for id in graph.adj.keys() {
        let label = get_label(id);
        writeln!(file, "  \"{}\";", label).unwrap();
    }

    // Keep track of drawn edges to avoid duplicates
    let mut drawn_edges = HashSet::new();

    for (from, neighbors) in &graph.adj {
        let from_label = get_label(from);
        for (to, _weight) in neighbors {
            let to_label = get_label(to);

            // Canonical edge tuple (min, max) so each edge is drawn once
            let edge = if from_label < to_label {
                (from_label.clone(), to_label.clone())
            } else {
                (to_label.clone(), from_label.clone())
            };

            if drawn_edges.insert(edge.clone()) {
                writeln!(file, "  \"{}\" -- \"{}\";", edge.0, edge.1).unwrap();
            }
        }
    }

    writeln!(file, "}}").unwrap();

    // Generate image with Graphviz
    let format = if output_file.ends_with(".png") {
        "png"
    } else if output_file.ends_with(".svg") {
        "svg"
    } else {
        panic!("Output file must end with .png or .svg");
    };

    let status = Command::new("dot")
        .arg(format!("-T{}", format))
        .arg(dot_file)
        .arg("-o")
        .arg(output_file)
        .status()
        .expect("Failed to run Graphviz. Make sure 'dot' is installed.");

    if status.success() {
        println!("Graph image generated at {}", output_file);
    } else {
        eprintln!("Graphviz failed with status: {:?}", status);
    }
}
