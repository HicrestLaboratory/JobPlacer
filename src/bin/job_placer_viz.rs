use clap::Parser;
use job_placer::{
    graph::display::{display_graph, DisplayOptions},
    init_logger,
    ir::id::Id,
    load_topology, resolve_nodes_filter, Cli,
};
use log::info;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    init_logger(cli.verbose);

    // -----------------------------------------------------------------------
    // Load topology
    // -----------------------------------------------------------------------
    let mut ir = load_topology(&cli)?;

    if !cli.all_nodes {
        // -----------------------------------------------------------------------
        // Resolve node allocation
        // -----------------------------------------------------------------------
        let allocated_hostnames = resolve_nodes_filter(&cli)?;
        info!("✓ Allocation: {} nodes", allocated_hostnames.len());
        let filter: Vec<Id> = allocated_hostnames
            .iter()
            .map(|n| Id::from(n.as_str()))
            .collect();

        // println!("{:#?}", candidate_anchors);

        ir = ir.filter_with_topology(&filter);
    }

    display_graph(&ir, "topo.svg", None, &DisplayOptions::default());

    Ok(())
}
