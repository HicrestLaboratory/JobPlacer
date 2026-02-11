use std::env;
use topology_extractor::parsers::leonardo;
use topology_extractor::builder::graph::graph_from_ir;
use topology_extractor::builder::display::display_graph;
use topology_extractor::parsers::yaml::save_ir_as_yaml;
use topology_extractor::ir::id::Id;
use topology_extractor::query::{TopologyQuery, Constraint, ReferencePoint, DistanceGroup, DistanceGroupWithParent};

fn main() {
    // Parse the topology
    let ir = topology_extractor::parsers::leonardo::from_file("../leo.txt");
    println!("Original topology: {} entities", ir.entities.len());
    
    let anchor_node = Id::from("lrdn4707");
    
    // Test 1: Regular DistanceGroup (no parent constraint)
    println!("\n========================================");
    println!("TEST 1: Regular Distance Groups");
    println!("========================================");
    
    let query1 = TopologyQuery::new()
        .with_constraint(Constraint::DistanceGroup {
            reference: ReferencePoint::First,
            groups: vec![
                DistanceGroup { count: 2, distance: 2.0 },
                DistanceGroup { count: 2, distance: 4.0 },
            ],
        });
    
    match query1.execute_from(&ir, anchor_node.clone()) {
        Ok(selected_nodes) => {
            println!("✓ Selected {} nodes:", selected_nodes.len());
            for (i, node) in selected_nodes.iter().enumerate() {
                println!("  {}: {}", i + 1, node.0);
            }
            
            // Visualize
            let filtered_ir = ir.filter_with_topology(&selected_nodes);
            let (graph, _) = graph_from_ir(&filtered_ir);
            display_graph(&graph, &filtered_ir, "query_test1_regular.svg");
            println!("Saved: query_test1_regular.svg");
        }
        Err(e) => println!("✗ Query failed: {}", e),
    }
    
    // Test 2: NodesAtDistanceWithSharedParent
    println!("\n========================================");
    println!("TEST 2: Nodes at Distance 4 with Shared Parent");
    println!("========================================");
    
    let query2 = TopologyQuery::new()
        .with_constraint(Constraint::NodesAtDistanceWithSharedParent {
            count: 2,
            distance: 4.0,
            reference: ReferencePoint::First,
            parent_level: 1, // Same L1 switch
        });
    
    match query2.execute_from(&ir, anchor_node.clone()) {
        Ok(selected_nodes) => {
            println!("✓ Selected {} nodes (must share same L1 switch):", selected_nodes.len());
            for (i, node) in selected_nodes.iter().enumerate() {
                println!("  {}: {}", i + 1, node.0);
            }
            
            // Visualize
            let filtered_ir = ir.filter_with_topology(&selected_nodes);
            let (graph, _) = graph_from_ir(&filtered_ir);
            display_graph(&graph, &filtered_ir, "query_test2_shared_parent.svg");
            println!("Saved: query_test2_shared_parent.svg");
        }
        Err(e) => println!("✗ Query failed: {}", e),
    }
    
    // Test 3: DistanceGroupWithSharedParent (multiple groups)
    println!("\n========================================");
    println!("TEST 3: Distance Groups with Shared Parent Constraint");
    println!("========================================");
    
    let query3 = TopologyQuery::new()
        .with_constraint(Constraint::DistanceGroupWithSharedParent {
            reference: ReferencePoint::First,
            groups: vec![
                DistanceGroupWithParent { count: 1, distance: 2.0, parent_level: 0 },
                DistanceGroupWithParent { count: 2, distance: 5.0, parent_level: 1 },
            ],
        });
    
    match query3.execute_from(&ir, anchor_node.clone()) {
        Ok(selected_nodes) => {
            println!("✓ Selected {} nodes:", selected_nodes.len());
            println!("  - 2 at distance 2.0 (sharing same L0 switch among themselves)");
            println!("  - 2 at distance 5.0 (sharing same L1 switch among themselves)");
            for (i, node) in selected_nodes.iter().enumerate() {
                println!("  {}: {}", i + 1, node.0);
            }
            
            // Visualize
            let filtered_ir = ir.filter_with_topology(&selected_nodes);
            let (graph, _) = graph_from_ir(&filtered_ir);
            display_graph(&graph, &filtered_ir, "query_test3_groups_shared_parent.svg");
            println!("Saved: query_test3_groups_shared_parent.svg");
        }
        Err(e) => println!("✗ Query failed: {}", e),
    }
    
    // Test 4: Complex mixed constraint
    println!("\n========================================");
    println!("TEST 4: Mixed Constraints");
    println!("========================================");
    println!("- 1 node at distance 2.0 (any)");
    println!("- 2 nodes at distance 4.0 (must share L1 switch)");
    
    let query4 = TopologyQuery::new()
        .with_constraint(Constraint::NodesAtDistance {
            count: 1,
            distance: 2.0,
            reference: ReferencePoint::First,
        })
        .with_constraint(Constraint::NodesAtDistanceWithSharedParent {
            count: 2,
            distance: 4.0,
            reference: ReferencePoint::First,
            parent_level: 1,
        });
    
    match query4.execute_from(&ir, anchor_node.clone()) {
        Ok(selected_nodes) => {
            println!("✓ Selected {} nodes:", selected_nodes.len());
            for (i, node) in selected_nodes.iter().enumerate() {
                println!("  {}: {}", i + 1, node.0);
            }
            
            // Visualize
            let filtered_ir = ir.filter_with_topology(&selected_nodes);
            let (graph, _) = graph_from_ir(&filtered_ir);
            display_graph(&graph, &filtered_ir, "query_test4_mixed.svg");
            println!("Saved: query_test4_mixed.svg");
        }
        Err(e) => println!("✗ Query failed: {}", e),
    }
    
    println!("\n========================================");
    println!("All tests completed!");
    println!("Check the generated SVG files:");
    println!("  - query_test1_regular.svg");
    println!("  - query_test2_shared_parent.svg");
    println!("  - query_test3_groups_shared_parent.svg");
    println!("  - query_test4_mixed.svg");
    println!("========================================");
}