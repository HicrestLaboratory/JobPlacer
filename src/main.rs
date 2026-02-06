use std::env;
use topology_extractor::parsers::leonardo;
use topology_extractor::builder::graph::graph_from_ir;
use topology_extractor::builder::display::display_graph;
use topology_extractor::parsers::yaml::save_ir_as_yaml;
use topology_extractor::ir::id::Id;

fn main() {
    let ir = topology_extractor::parsers::leonardo::from_file("../leo.txt");
    
    let target_compute_nodes = vec![
        Id::from("lrdn2983"),
        Id::from("lrdn2654"),
        Id::from("lrdn3579"),
        Id::from("lrdn1022"),
        Id::from("lrdn2054"),
        Id::from("lrdn0860"),
        Id::from("lrdn1633"),
        Id::from("lrdn1404"),
        Id::from("lrdn3954"),
        Id::from("lrdn0240"),
        Id::from("lrdn2659"),
        Id::from("lrdn1611"),
        Id::from("lrdn0350"),
        Id::from("lrdn1185"),
        Id::from("lrdn1909"),
        Id::from("lrdn2954"),
        Id::from("lrdn1707"),
        Id::from("lrdn0216"),
        Id::from("lrdn1184"),
        Id::from("lrdn1854"),
        Id::from("lrdn2695"),
        Id::from("lrdn0665"),
        Id::from("lrdn0713"),
        Id::from("lrdn3685"),
        Id::from("lrdn4004"),
        Id::from("lrdn1958"),
        Id::from("lrdn4128"),
        Id::from("lrdn4649"),
        Id::from("lrdn3657"),
        Id::from("lrdn4655"),
        Id::from("lrdn0641"),
        Id::from("lrdn4320"),
        Id::from("lrdn0745"),
    ];
    
    let filtered_ir = ir.filter_with_topology(&target_compute_nodes);
    
    println!("Original: {} entities", ir.entities.len());
    println!("Filtered: {} entities (compute nodes + topology)", filtered_ir.entities.len());
    
    save_ir_as_yaml(&filtered_ir, "NANJING_filtered.yaml").expect("Failed to save YAML");
    
    let (graph, _indices) = graph_from_ir(&filtered_ir);
    display_graph(&graph, &filtered_ir, "nanjing_filtered.svg");
    
    println!("Graph generated: nanjing_filtered.svg ({} nodes)", graph.node_count());
}