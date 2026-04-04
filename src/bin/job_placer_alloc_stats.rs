use clap::Parser;
use job_placer::{
    graph::display::{display_graph, Allocations, DisplayOptions},
    init_logger,
    ir::Id,
    load_topology,
    placement::filter_ir_by_allocations,
    placement_stats::PlacementStats,
    resolve_nodes_filter, Cli,
};
use log::info;
use std::{
    collections::BTreeMap,
    io::{self, Read},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    init_logger(cli.verbose);

    // -----------------------------------------------------------------------
    // Load topology
    // -----------------------------------------------------------------------
    let mut ir = load_topology(&cli)?;

    if !cli.all_nodes {
        let allocated_hostnames = resolve_nodes_filter(&cli)?;
        info!("✓ Allocation: {} nodes", allocated_hostnames.len());
        let filter: Vec<Id> = allocated_hostnames
            .iter()
            .map(|n| Id::from(n.as_str()))
            .collect();
        ir = ir.filter_with_topology(&filter);
    } else {
        info!("You forces using ALL nodes");
    }

    // -----------------------------------------------------------------------
    // Parse placement query
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
    let allocations: BTreeMap<String, Vec<String>> = serde_json::from_str(json_content.as_str())?;
    info!("✓ Allocations: {} jobs", allocations.len());

    // -----------------------------------------------------------------------
    // Get stats
    // -----------------------------------------------------------------------
    let stats = PlacementStats::compute(&ir, &allocations);
    println!("{}", serde_json::to_string_pretty(&stats)?);

    if cli.visualize || cli.out_svg.is_some() {
        let alloc: Allocations = allocations
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect();
        display_graph(
            &filter_ir_by_allocations(&ir, &alloc),
            if let Some(f) = cli.out_svg {
                f
            } else {
                String::from("allocation.svg")
            }
            .as_str(),
            Some(&alloc),
            &DisplayOptions::default(),
        );
    }

    Ok(())
}
