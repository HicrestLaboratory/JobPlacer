use petgraph::graph::Graph as PetGraph;
use petgraph::visit::EdgeRef;
use crate::ir::topology_ir::TopologyIR;
use crate::ir::id::Id;
use std::collections::HashSet;
use std::io::Write; // <-- this is required

pub fn display_graph<Ty>(graph: &PetGraph<Id, f32, Ty>, ir: &TopologyIR, output_file: &str)
where
    Ty: petgraph::EdgeType,
{
    let dot_file = "topology.dot";
    let mut file = std::fs::File::create(dot_file).expect("Failed to create DOT file");

    writeln!(file, "graph G {{").unwrap();
    writeln!(file, "  rankdir=TB;").unwrap();
    writeln!(
        file,
        "  node [shape=box, style=filled, color=lightblue, fontname=\"Arial\"];\n"
    )
    .unwrap();
    writeln!(file, "  edge [dir=none, penwidth=2, color=black];\n").unwrap();

    let get_label = |id: &Id| -> String {
        match ir.entities.get(id) {
            Some(entity) => match &entity.kind {
                crate::ir::entity::EntityKind::Compute => id.0.clone(),
                crate::ir::entity::EntityKind::Group => format!("Group {}", id.0),
                crate::ir::entity::EntityKind::Switch { level } => {
                    format!("{} (L{})", id.0, level.unwrap_or(0))
                }
            },
            None => id.0.clone(),
        }
    };

    let mut drawn_edges = HashSet::new();

    for edge in graph.edge_references() {
        let a = &graph[edge.source()];
        let b = &graph[edge.target()];
        let edge_tuple = if a.0 < b.0 { (a.0.clone(), b.0.clone()) } else { (b.0.clone(), a.0.clone()) };

        if drawn_edges.insert(edge_tuple.clone()) {
            writeln!(file, "  \"{}\" -- \"{}\";", get_label(a), get_label(b)).unwrap();
        }
    }

    // Write all nodes to show isolated ones
    for node_idx in graph.node_indices() {
        let id = &graph[node_idx];
        writeln!(file, "  \"{}\";", get_label(id)).unwrap();
    }

    writeln!(file, "}}").unwrap();

    let format = if output_file.ends_with(".png") {
        "png"
    } else if output_file.ends_with(".svg") {
        "svg"
    } else {
        panic!("Output must end with .png or .svg");
    };

    let status = std::process::Command::new("dot")
        .arg(format!("-T{}", format))
        .arg(dot_file)
        .arg("-o")
        .arg(output_file)
        .status()
        .expect("Failed to run Graphviz");

    if status.success() {
        println!("Graph image generated at {}", output_file);
    } else {
        eprintln!("Graphviz failed");
    }
}
