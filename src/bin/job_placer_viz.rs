use std::io::{self, Read};

use clap::Parser;
use job_placer::{
    graph::display::{display_graph, Allocations, DisplayOptions},
    init_logger,
    ir::Id,
    load_topology, resolve_nodes_filter, Cli,
};
use log::info;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    init_logger(cli.verbose);

    let allocations = if cli.wait_stdin {
        info!("Reading query from stdin…");
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        let parsed: Allocations = serde_json::from_str(&buf)?;
        Some(parsed)
    } else {
        None
    };

    // -----------------------------------------------------------------------
    // Load topology
    // -----------------------------------------------------------------------
    let mut ir = load_topology(&cli)?;

    if !cli.all_nodes {
        // -----------------------------------------------------------------------
        // Resolve node allocation
        // -----------------------------------------------------------------------
        let allocated_hostnames = resolve_nodes_filter(&cli)?;
        info!("Original allocation: {} nodes", allocated_hostnames.len());
        let filter: Vec<Id> = allocated_hostnames
            .iter()
            .map(|n| Id::from(n.as_str()))
            .collect();

        // println!("{:#?}", candidate_anchors);

        ir = ir.filter_with_topology(&filter);
    }

    if let Some(all) = allocations.as_ref() {
        info!("User-provided allocations: {:?}", all);
    }

    display_graph(
        &ir,
        cli.out_svg
            .unwrap_or(format!("topo_{}.svg", cli.system))
            .as_str(),
        allocations.as_ref(),
        &DisplayOptions::default(),
    );

    Ok(())
}
