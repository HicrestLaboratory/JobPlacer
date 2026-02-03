use std::env;
use topology_extractor::parsers::manual;
use topology_extractor::builder::{graph::Graph, display, validate};
use topology_extractor::builder::display::generate_graph_image;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <topology_file.yaml>", args[0]);
        std::process::exit(1);
    }

    let filename = &args[1];

    // Parse the YAML into the intermediate representation
    let ir = manual::from_file(filename);

    // Validate the IR
    validate::validate(&ir).expect("Topology validation failed");

    let graph = Graph::from_ir(&ir);
    println!("Graph has {} vertices", graph.adj.len());

    // generate PNG
    generate_graph_image(&graph, &ir, &(filename.to_string() + ".png"));

    // or generate SVG
    generate_graph_image(&graph, &ir, &(filename.to_string() + ".svg"));
}