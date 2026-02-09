use std::env;
use topology_extractor::parsers::leonardo;
use topology_extractor::parsers::manual::from_file;
use topology_extractor::builder::graph::graph_from_ir;
use topology_extractor::builder::display::display_graph;
use topology_extractor::parsers::yaml::save_ir_as_yaml;
use topology_extractor::ir::id::Id;
use topology_extractor::query::{TopologyQuery, Constraint, ReferencePoint, DistanceGroup};

fn main() {
    // Parse the topology
    let ir = topology_extractor::parsers::manual::from_file("./clusters/NANJING.yaml");
    println!("Original topology: {} entities", ir.entities.len());
    
    // Create a query: 2 nodes at distance 2, 2 nodes at distance 4 from first node
    let query = TopologyQuery::new()
        .with_constraint(Constraint::DistanceGroup {
            reference: ReferencePoint::First,
            groups: vec![
                DistanceGroup { count: 1, distance: 2.0 },  // Same L1 switch
                DistanceGroup { count: 2, distance: 4.0 },  // Different L1 switch
            ],
        });
    
    // Execute query with a specific anchor node (change "cn1" to an actual compute node ID from your topology)
    let anchor_node = Id::from("cn1"); // Replace with actual compute node ID
    let selected_nodes = query.execute_from(&ir, anchor_node.clone())
        .expect("Query execution failed");
    
    println!("\nQuery Results:");
    println!("Selected {} compute nodes:", selected_nodes.len());
    
    let node_ids: Vec<_> = selected_nodes.iter().map(|node_id| node_id.0.clone()).collect();
    
    // Filter IR to include selected nodes + their topology (switches, etc.)
    let filtered_ir = ir.filter_with_topology(&selected_nodes);
    println!("\nFiltered topology: {} entities (compute nodes + switches)", filtered_ir.entities.len());

    for node_id in &node_ids {
        println!(" - {}", node_id);
    }
    
    // Save filtered IR as YAML
    save_ir_as_yaml(&filtered_ir, "query_result.yaml")
        .expect("Failed to save YAML");
    println!("Saved topology to: query_result.yaml");
    
    // Generate and display SVG graph
    let (graph, _indices) = graph_from_ir(&filtered_ir);
    display_graph(&graph, &filtered_ir, "query_result.svg");
}