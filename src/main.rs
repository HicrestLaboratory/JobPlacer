use std::env;
use topology_extractor::parsers::leonardo;
use topology_extractor::builder::graph::graph_from_ir;
use topology_extractor::builder::display::display_graph;
use topology_extractor::parsers::yaml::save_ir_as_yaml;

fn main() {
    // Get the input file from command line
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: cargo run <topology_file>");
        std::process::exit(1);
    }
    let path = &args[1];

    // Parse the topology
    let ir = leonardo::from_file(path);

    // Save IR as YAML
    let yaml_path = "LEONARDO.yaml";
    save_ir_as_yaml(&ir, yaml_path).expect("Failed to save YAML");
    println!("IR saved to {}", yaml_path);

    // Build PetGraph
    let (graph, _indices) = graph_from_ir(&ir);

    // Display / generate image
    let output_image = "leonardo.svg";
    display_graph(&graph, &ir, output_image);
    println!("Graph image generated: {}", output_image);
}
