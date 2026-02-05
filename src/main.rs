use topology_extractor::parsers::leonardo;
use topology_extractor::builder::{graph::Graph, display, validate};
use topology_extractor::builder::display::generate_graph_image;

fn main() {
    println!("Fetching Leonardo topology from scontrol...");
    
    // Parse the topology directly from scontrol command
    let ir = leonardo::from_scontrol();
    
    println!("Topology fetched successfully!");
    
    // Validate the IR
    match validate::validate(&ir) {
        Ok(_) => println!("Topology validation passed!"),
        Err(e) => {
            eprintln!("Topology validation failed: {}", e);
            std::process::exit(1);
        }
    }
    
    // Build the graph
    let graph = Graph::from_ir(&ir);
    println!("Graph has {} vertices", graph.adj.len());
    
    // Generate visualization files
    let output_base = "leonardo_topology";
    
    println!("Generating PNG...");
    generate_graph_image(&graph, &ir, &format!("{}.png", output_base));
    
    println!("Generating SVG...");
    generate_graph_image(&graph, &ir, &format!("{}.svg", output_base));
    
    println!("Done! Output files:");
    println!("  - {}.png", output_base);
    println!("  - {}.svg", output_base);
}