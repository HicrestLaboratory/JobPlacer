use std::io::{self, Read};
use std::path::PathBuf;
use std::process::Command;

use clap::{Args, Parser};
use serde::Deserialize;

use job_placer::ir::id::Id;
use job_placer::parsers::{leonardo, manual};
use job_placer::query::{Constraint, DistanceGroup, ReferencePoint, TopologyQuery};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Place jobs on the cluster topology according to a set of constraints.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the query JSON file. If omitted, the query is read from stdin.
    query: Option<PathBuf>,

    /// Explicit node list (comma- or newline-separated hostnames).
    /// Required when not running inside a SLURM allocation.
    #[arg(short = 'n', long, value_name = "HOSTNAMES")]
    nodelist: Option<String>,

    /// Enable informational/debug output. By default only the result is printed.
    #[arg(short = 'v', long)]
    verbose: bool,

    /// How to load the system topology.
    #[command(flatten)]
    topology: TopologyArgs,

    /// System-specific topology source (used together with --system).
    #[command(flatten)]
    topo_source: TopoSource,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
struct TopologyArgs {
    /// Path to a YAML topology file (parsed with `manual::from_file`).
    #[arg(short = 'f', long, value_name = "PATH", group = "topo")]
    topology_file: Option<PathBuf>,

    /// Use a named system together with --topology-system-file or
    /// --topology-scontrol to specify the data source.
    /// Currently supported: leonardo
    #[arg(short = 's', long, value_name = "SYSTEM", requires = "topo_source")]
    system: Option<String>,
}

/// Secondary group that is only meaningful when --system is given.
#[derive(Args, Debug)]
#[group(id = "topo_source", multiple = false)]
struct TopoSource {
    /// Path to the system-specific topology file.
    #[arg(short = 'F', long, value_name = "PATH")]
    topology_system_file: Option<PathBuf>,

    /// Discover topology via `scontrol` instead of a file.
    #[arg(short = 'S', long)]
    topology_scontrol: bool,
}

// ---------------------------------------------------------------------------
// JSON input structures
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
struct QueryInput {
    constraints: Vec<ConstraintInput>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum ConstraintInput {
    NodesAtDistance {
        count: usize,
        distance: f32,
        reference: String,
    },
    NodesAtDistanceWithSharedParent {
        count: usize,
        distance: f32,
        reference: String,
        parent_level: usize,
    },
    DistanceGroup {
        reference: String,
        groups: Vec<DistanceGroupInput>,
    },
}

#[derive(Deserialize, Debug)]
struct DistanceGroupInput {
    count: usize,
    distance: f32,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI *first* so --help / --version work without any side-effects.
    let cli = Cli::parse();

    // -----------------------------------------------------------------------
    // 1. Read query JSON
    // -----------------------------------------------------------------------
    let json_content = match &cli.query {
        Some(path) => {
            info(&cli, &format!("Loading query from: {}", path.display()));
            std::fs::read_to_string(path)?
        }
        None => {
            info(&cli, "Reading query from stdin…");
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };

    let query_input: QueryInput = serde_json::from_str(&json_content)?;

    // -----------------------------------------------------------------------
    // 2. Resolve node allocation
    // -----------------------------------------------------------------------
    let allocated_hostnames = resolve_nodes(&cli)?;
    info(
        &cli,
        &format!("✓ Allocation: {} nodes", allocated_hostnames.len()),
    );
    let allocated_ids: Vec<Id> = allocated_hostnames
        .iter()
        .map(|n| Id::from(n.as_str()))
        .collect();

    // -----------------------------------------------------------------------
    // 3. Load topology
    // -----------------------------------------------------------------------
    let full_ir = load_topology(&cli)?;
    let my_allocation_ir = full_ir.filter_with_topology(&allocated_ids);

    // -----------------------------------------------------------------------
    // 4. Build query from JSON
    // -----------------------------------------------------------------------
    let mut query = TopologyQuery::new();

    for c in query_input.constraints {
        let constraint = match c {
            ConstraintInput::NodesAtDistance {
                count,
                distance,
                reference,
            } => Constraint::NodesAtDistance {
                count,
                distance,
                reference: parse_ref(&reference),
            },
            ConstraintInput::NodesAtDistanceWithSharedParent {
                count,
                distance,
                reference,
                parent_level,
            } => Constraint::NodesAtDistanceWithSharedParent {
                count,
                distance,
                reference: parse_ref(&reference),
                parent_level,
            },
            ConstraintInput::DistanceGroup { reference, groups } => {
                let parsed_groups = groups
                    .iter()
                    .map(|g| DistanceGroup {
                        count: g.count,
                        distance: g.distance,
                    })
                    .collect();

                Constraint::DistanceGroup {
                    reference: parse_ref(&reference),
                    groups: parsed_groups,
                }
            }
        };
        query = query.with_constraint(constraint);
    }

    // -----------------------------------------------------------------------
    // 5. Execute search
    // -----------------------------------------------------------------------
    for anchor in &allocated_ids {
        if let Ok(selected_nodes) = query.execute_from(&my_allocation_ir, anchor.clone()) {
            let result_str = selected_nodes
                .iter()
                .map(|id| id.0.clone())
                .collect::<Vec<_>>()
                .join(",");

            info(&cli, &format!("   Nodes: {}", result_str));
            // Machine-readable tag always emitted (it IS the result).
            let output = if cli.verbose { format!("::RESULT::{}", result_str) } else { result_str };
            println!("{}", output);
            return Ok(());
        }
    }

    eprintln!("❌ Topology search failed: no valid placement found.");
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Print a message only when --verbose is set.
fn info(cli: &Cli, msg: &str) {
    if cli.verbose {
        println!("{}", msg);
    }
}

/// Resolve the list of hostnames to use, either from SLURM or from --nodelist.
fn resolve_nodes(cli: &Cli) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Try SLURM first.
    if let Ok(nodelist_env) = std::env::var("SLURM_JOB_NODELIST") {
        info(cli, "Detected SLURM environment, expanding node list…");
        let output = Command::new("scontrol")
            .args(["show", "hostnames", &nodelist_env])
            .output()?;
        let stdout = String::from_utf8(output.stdout)?;
        let nodes: Vec<String> = stdout
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !nodes.is_empty() {
            return Ok(nodes);
        }
    }

    // Fall back to --nodelist.
    if let Some(raw) = &cli.nodelist {
        info(cli, "Using explicit --nodelist.");
        // Accept comma- or newline-separated hostnames.
        let nodes: Vec<String> = raw
            .split([',', '\n'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return Ok(nodes);
    }

    // Neither available.
    eprintln!(
        "error: Not inside a SLURM allocation and --nodelist was not provided.\n\
         Provide --nodelist <HOSTNAMES> or run inside a SLURM job."
    );
    std::process::exit(1);
}

/// Load the topology IR according to the CLI flags.
fn load_topology(cli: &Cli) -> Result<job_placer::ir::topology_ir::TopologyIR, Box<dyn std::error::Error>> {
    let topo = &cli.topology;

    // Option A: plain YAML file.
    if let Some(path) = &topo.topology_file {
        info(cli, &format!("Loading topology from YAML: {}", path.display()));
        return Ok(manual::from_file(path));
    }

    // Option B / C: named system + source.
    if let Some(system) = &topo.system {
        match system.to_lowercase().as_str() {
            "leonardo" => {
                if cli.topo_source.topology_scontrol {
                    info(cli, "Loading Leonardo topology via scontrol...");
                    return Ok(leonardo::from_scontrol());
                }
                if let Some(path) = &cli.topo_source.topology_system_file {
                    info(
                        cli,
                        &format!("Loading Leonardo topology from file: {}", path.display()),
                    );
                    return Ok(leonardo::from_file(path));
                }
                // clap's `requires` group should prevent this, but be safe.
                eprintln!(
                    "error: --system leonardo requires either \
                     --topology-system-file <PATH> or --topology-scontrol."
                );
                std::process::exit(1);
            }
            other => {
                eprintln!("error: Unknown system '{}'. Supported: leonardo.", other);
                std::process::exit(1);
            }
        }
    }

    unreachable!("clap's required group guarantees one topology flag is set");
}

fn parse_ref(s: &str) -> ReferencePoint {
    match s.to_lowercase().as_str() {
        _ => ReferencePoint::First,
    }
}