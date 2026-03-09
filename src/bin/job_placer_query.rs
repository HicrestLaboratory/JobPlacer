use std::io::{self, Read};
use std::str::FromStr;

use clap::Parser;
use job_placer::{init_logger, load_topology, resolve_nodes_filter, Cli};
use log::info;

use job_placer::ir::id::Id;
use job_placer::query::TopologyQuery;

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

        ir = ir.filter_with_topology(&candidate_anchors);
    }

    // -----------------------------------------------------------------------
    // Read, Build and Execute Query
    // -----------------------------------------------------------------------
    let json_content = match &cli.query {
        Some(path) => {
            info!("Loading query from: {}", path.display());
            std::fs::read_to_string(path)?
        }
        None => {
            info!("Reading query from stdin…");
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    let query = TopologyQuery::from_str(&json_content)?;

    for anchor in &candidate_anchors {
        if let Ok(selected_nodes) = query.execute_from(&ir, anchor.clone()) {
            let result_str = selected_nodes
                .iter()
                .map(|id| id.0.clone())
                .collect::<Vec<_>>()
                .join(",");

            info!("Nodes: {}", result_str);
            println!("{}", result_str);
            return Ok(());
        }
    }

    eprintln!("❌ Topology search failed: no valid placement found.");
    std::process::exit(1);
}
