use clap::{Parser};
use job_placer::{Cli, graph::{display::display_graph, graph::graph_from_ir}, init_logger, ir::id::Id, load_topology, resolve_nodes_filter};
use log::info;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    init_logger(cli.verbose);

    // -----------------------------------------------------------------------
    // Load topology
    // -----------------------------------------------------------------------
    let mut ir = load_topology(&cli)?;
    let mut candidate_anchors: Vec<Id> = ir.entities.keys().map(|id| id.clone()).collect();

    if !cli.all_nodes {
        // -----------------------------------------------------------------------
        // Resolve node allocation
        // -----------------------------------------------------------------------
        let allocated_hostnames = resolve_nodes_filter(&cli)?;
        info!("✓ Allocation: {} nodes", allocated_hostnames.len());
        candidate_anchors = allocated_hostnames
            .iter()
            .map(|n| Id::from(n.as_str()))
            .collect();

        // println!("{:#?}", candidate_anchors);

        ir = ir.filter_with_topology(&candidate_anchors);
    }

    let graph = graph_from_ir(&ir);
    display_graph(&graph.0, &ir, "topo.svg");

    Ok(())
}