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

use job_placer::placement::{JobRequest, PlacementResult, PlacementStrategy, Placer};

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
    //
    // Retry schedule (Auto mode):
    //
    //   Tier 1 — Strict:      honour every constraint exactly.
    //   Tier 2 — Relaxed:     IntraGroup may spill cross-cell.
    //   Tier 3 — BestEffort:  additionally drops block-alignment when needed.
    //
    // Each tier uses its own seed range so seed variance and constraint
    // relaxation are orthogonal.  A fixed --strategy skips straight to that
    // tier for all attempts.
    // -----------------------------------------------------------------------

    // (strategy, seed_offset) pairs — built once, iterated in order.
    // Each tier gets ATTEMPTS_PER_TIER seed variants before escalating.
    const ATTEMPTS_PER_TIER: usize = 7;

    #[rustfmt::skip]
    let schedule: &[(PlacementStrategy, u64)] = &[
        (PlacementStrategy::Strict,      0),
        (PlacementStrategy::Relaxed,     ATTEMPTS_PER_TIER as u64),
        (PlacementStrategy::BestEffort,  ATTEMPTS_PER_TIER as u64 * 2),
    ];

    let base_seed = cli.seed.unwrap_or(42);
    let total_attempts = schedule.len() * ATTEMPTS_PER_TIER;

    let mut placer = Placer::new(&ir, base_seed);
    let mut last_result: Option<PlacementResult> = None;
    let mut global_attempt = 0usize;

    for &(strategy, seed_offset) in schedule {
        info!("── Strategy tier: {strategy:?} ({ATTEMPTS_PER_TIER} attempt(s))");

        for i in 0..ATTEMPTS_PER_TIER {
            global_attempt += 1;
            let seed = base_seed.wrapping_add(seed_offset + i as u64);
            placer.change_seed(seed);

            info!(
                "Attempt #{global_attempt}/{total_attempts} [strategy={strategy:?}, seed={seed}]"
            );

            let result = placer.place_with_strategy(&jobs, strategy);

            match result {
                PlacementResult::Ok { ref placements } => {
                    // Warn on stderr for any job whose constraint was relaxed,
                    // while keeping stdout clean for downstream consumers.
                    let relaxed: Vec<_> = placements
                        .iter()
                        .filter(|(_, p)| p.placement_class != p.achieved_class)
                        .collect();

                    if !relaxed.is_empty() {
                        eprintln!(
                            "WARNING: {n} job(s) placed with relaxed constraints \
                             [strategy={strategy:?}, seed={seed}]:",
                            n = relaxed.len()
                        );
                        for (name, p) in &relaxed {
                            eprintln!(
                                "  {name}: requested={req}  achieved={ach}",
                                req = p.placement_class,
                                ach = p.achieved_class,
                            );
                        }
                    }

                    println!("{}", serde_json::to_string_pretty(&result)?);

                    if cli.visualize || cli.out_svg.is_some() {
                        if let Some(allocations) = placement_to_allocations(&result) {
                            display_graph(
                                &filter_ir_by_allocations(&ir, &allocations),
                                if let Some(ref f) = cli.out_svg {
                                    f.as_str()
                                } else {
                                    "topology_placement.svg"
                                },
                                Some(&allocations),
                                &DisplayOptions::default(),
                            );
                        }
                    }

                    std::process::exit(0);
                }

                PlacementResult::Infeasible { ref reason } => {
                    info!(
                        "Attempt #{global_attempt} failed [strategy={strategy:?}, seed={seed}]: \
                         {reason}"
                    );
                    last_result = Some(result);
                }
            }
        }
    }

    // All attempts exhausted — report the last failure and exit non-zero.
    let failed = last_result.unwrap();
    eprintln!(
        "ERROR: placement failed after {total_attempts} attempt(s) \
         (final tier: BestEffort)"
    );
    println!("{}", serde_json::to_string_pretty(&failed)?);
    std::process::exit(1);
}