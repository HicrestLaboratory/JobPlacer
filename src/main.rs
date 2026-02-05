use std::env;

use topology_extractor::parsers::leonardo;
use topology_extractor::builder::{graph::Graph, validate};
use topology_extractor::builder::display::generate_graph_image;
use topology_extractor::parsers::yaml::save_ir_as_yaml;

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = &args[1];

    // Parse the topology from file for testing
    let ir = leonardo::from_file(path);

    println!("Topology fetched successfully!");

    // Validate the IR
    match validate::validate(&ir) {
        Ok(_) => println!("Topology validation passed!"),
        Err(e) => {
            eprintln!("Topology validation failed: {}", e);
            std::process::exit(1);
        }
    }

    // Save IR as YAML
    let yaml_path = "LEONARDO.yaml";
    save_ir_as_yaml(&ir, yaml_path).expect("Failed to save YAML");
    println!("Topology saved to {}", yaml_path);

    // Build the graph
    let graph = Graph::from_ir(&ir);
    println!("Graph has {} vertices", graph.adj.len());

    println!("Done!");
}
