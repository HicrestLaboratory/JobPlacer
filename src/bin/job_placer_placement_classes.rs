use clap::Parser;
use job_placer::{
    graph::display::{display_graph, Allocations, DisplayOptions},
    init_logger,
    ir::{entity::EntityKind, id::Id, topology_ir::TopologyIR},
    load_topology, resolve_nodes_filter, Cli,
};
use log::info;
use std::{
    collections::{BTreeMap, HashSet},
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

pub fn filter_ir_by_allocations(ir: &TopologyIR, allocations: &Allocations) -> TopologyIR {
    let allocated_nodes: HashSet<Id> = allocations
        .values()
        .flatten()
        .map(|s| Id(s.clone()))
        .collect();

    // Keep an L1 switch only if it contains at least one allocated node
    let active_l1s: HashSet<Id> = ir
        .entities
        .values()
        .filter(|e| matches!(e.kind, EntityKind::Switch { level: Some(0) }))
        .filter(|e| {
            ir.contains
                .get(&e.id)
                .map(|children| children.iter().any(|c| allocated_nodes.contains(c)))
                .unwrap_or(false)
        })
        .map(|e| e.id.clone())
        .collect();

    // Keep: all allocated compute nodes + active L1s + all L2 switches
    // (L2s are kept unconditionally since they represent fabric structure,
    //  not compute assignment — filter them out too if you prefer)
    let keep: Vec<Id> = ir
        .entities
        .keys()
        .filter(|id| {
            allocated_nodes.contains(id)
                || active_l1s.contains(id)
                || matches!(
                    ir.entities.get(id),
                    Some(e) if matches!(e.kind, EntityKind::Switch { level: Some(1) })
                )
        })
        .cloned()
        .collect();

    ir.filter_by_ids(&keep)
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
    let seed = cli.seed.unwrap_or(42);
    info!("Using seed: {seed}");
    let mut placer = Placer::new(&ir, seed);
    let result = placer.place(&jobs);

    println!("{}", serde_json::to_string_pretty(&result)?);

    if matches!(result, PlacementResult::Infeasible { .. }) {
        std::process::exit(1);
    }

    if let Some(allocations) = placement_to_allocations(&result) {
        display_graph(
            &filter_ir_by_allocations(&ir, &allocations),
            "topology_placement.svg",
            Some(&allocations),
            &DisplayOptions::default(),
        );
    }

    Ok(())
}
