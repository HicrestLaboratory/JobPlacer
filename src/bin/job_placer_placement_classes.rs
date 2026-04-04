use clap::Parser;
use job_placer::{
    graph::display::{display_graph, Allocations, DisplayOptions},
    init_logger,
    ir::Id,
    load_topology,
    placement::filter_ir_by_allocations,
    resolve_nodes_filter, Cli,
};
use log::info;
use std::{
    collections::BTreeMap,
    io::{self, Read},
};

use job_placer::placement::{JobRequest, PlacementResult, Placer};

pub fn placement_to_allocations(result: &PlacementResult) -> Option<Allocations> {
    match result {
        PlacementResult::Ok { placements } => Some(
            placements
                .iter()
                .map(|(job_name, placement)| {
                    (job_name.clone(), placement.nodes.iter().cloned().collect())
                })
                .collect(),
        ),
        PlacementResult::Infeasible { .. } => None,
    }
}

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
    let jobs: BTreeMap<String, JobRequest> = serde_json::from_str(json_content.as_str())?;
    info!("✓ Placement query: {} jobs", jobs.len());

    // -----------------------------------------------------------------------
    // Run placer
    // -----------------------------------------------------------------------
    let mut seed = cli.seed.unwrap_or(42);
    const ATTEMPTS: usize = 20;

    let mut last_result: Option<PlacementResult> = None;
    let mut placer = Placer::new(&ir, seed);

    for attempt in 0..ATTEMPTS {
        info!("Attempt #{}, using seed: {seed}", attempt + 1);
        let result = placer.place(&jobs);

        match result {
            PlacementResult::Ok { .. } => {
                println!("{}", serde_json::to_string_pretty(&result)?);

                if cli.visualize || cli.out_svg.is_some() {
                    if let Some(allocations) = placement_to_allocations(&result) {
                        display_graph(
                            &filter_ir_by_allocations(&ir, &allocations),
                            if let Some(f) = cli.out_svg {
                                f
                            } else {
                                String::from("topology_placement.svg")
                            }
                            .as_str(),
                            Some(&allocations),
                            &DisplayOptions::default(),
                        );
                    }
                }

                std::process::exit(0);
            }
            PlacementResult::Infeasible { ref reason } => {
                info!("Attempt #{} failed: {reason}", attempt + 1);
                last_result = Some(result);
                // Each retry uses a different seed so the random shuffle
                // explores a different region of the placement space.
                seed = seed.wrapping_add(1);
                placer.change_seed(seed);
            }
        }
    }

    // All attempts exhausted — print the last failure and exit with an error.
    let failed = last_result.unwrap();
    println!("{}", serde_json::to_string_pretty(&failed)?);
    std::process::exit(1);
}
