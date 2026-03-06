use clap::clap_derive::Parser;
use clap::Args;
use log::{LevelFilter, info};
use std::path::PathBuf;

use crate::{ir::topology_ir::TopologyIR, parsers::{jupiter, leonardo, manual, slurm::{expand_nodelist, get_nodelist_from_env}}};

pub mod graph;
pub mod ir;
pub mod parsers;
pub mod query;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Place jobs on the cluster topology according to a set of constraints.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Path to the query JSON file. If omitted, the query is read from stdin.
    pub query: Option<PathBuf>,

    /// Explicit node list (comma- or newline-separated hostnames).
    /// Required when not running inside a SLURM allocation.
    #[arg(short = 'n', long, value_name = "HOSTNAMES")]
    pub nodelist: Option<String>,

    /// Consider all available nodes
    #[arg(short = 'a', long)]
    pub all_nodes: bool,

    /// Enable info!rmational/debug output. By default only the result is printed.
    #[arg(short = 'v', long)]
    pub verbose: bool,

    /// How to load the system topology.
    #[command(flatten)]
    pub topology: TopologyArgs,

    /// System-specific topology source (used together with --system).
    #[command(flatten)]
    pub topo_source: TopoSource,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct TopologyArgs {
    /// Path to a YAML topology file (parsed with `manual::from_file`).
    #[arg(short = 'f', long, value_name = "PATH", group = "topo")]
    pub topology_yaml: Option<PathBuf>,

    /// Use a named system together with --topology-system-file or
    /// --topology-scontrol to specify the data source.
    /// Currently supported: leonardo
    #[arg(short = 's', long, value_name = "SYSTEM", requires = "topo_source")]
    pub system: Option<String>,
}

/// Secondary group that is only meaningful when --system is given.
#[derive(Args, Debug)]
#[group(id = "topo_source", multiple = false)]
pub struct TopoSource {
    /// Path to the system-specific topology file.
    #[arg(short = 'F', long, value_name = "PATH")]
    pub topology_file: Option<PathBuf>,

    /// Discover topology via `scontrol` instead of a file.
    #[arg(short = 'S', long)]
    pub topology_scontrol: bool,
}

pub fn init_logger(verbose: bool) {
    let level = if verbose {
        LevelFilter::Info
    } else {
        LevelFilter::Warn
    };

    env_logger::Builder::new().filter_level(level).init();
}

/// Load the topology IR according to the CLI flags.
pub fn load_topology(
    cli: &Cli,
) -> Result<TopologyIR, Box<dyn std::error::Error>> {
    let topo = &cli.topology;
    let supported_systems = ["leonardo", "jupiter"];

    // Option A: plain YAML file.
    if let Some(path) = &topo.topology_yaml {
        info!("Loading topology from YAML: {}", path.display());
        return Ok(manual::from_file(path));
    }

    // Option B / C: named system + source.
    if let Some(system) = &topo.system {
        if cli.topo_source.topology_scontrol {
            info!(
                "Loading {} topology via `scontrol show topology`...",
                system.to_uppercase().as_str()
            );
            return match system.to_lowercase().as_str() {
                "leonardo" => Ok(leonardo::from_scontrol()?),
                "jupiter" => Ok(jupiter::from_scontrol()?),
                other => {
                    eprintln!(
                        "error: Unknown system '{}'. Supported: {}.",
                        other,
                        supported_systems.join(", ")
                    );
                    std::process::exit(1);
                }
            };
        }
        if let Some(path) = &cli.topo_source.topology_file {
            info!(
                "Loading {} topology from file: {}",
                system.to_uppercase().as_str(),
                path.display()
            );
            return match system.to_lowercase().as_str() {
                "leonardo" => Ok(leonardo::from_file(path)?),
                "jupiter" => Ok(jupiter::from_file(path)?),
                other => {
                    eprintln!(
                        "error: Unknown system '{}'. Supported: {}.",
                        other,
                        supported_systems.join(", ")
                    );
                    std::process::exit(1);
                }
            };
        }
        
        eprintln!(
            "error: required either --topology-file <PATH> or --topology-scontrol."
        );
        std::process::exit(1);
    }

    unreachable!("clap's required group guarantees one topology flag is set");
}

/// Resolve the list of hostnames to use, either from SLURM or from --nodelist.
pub fn resolve_nodes_filter(cli: &Cli) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Try SLURM first.
    match get_nodelist_from_env() {
        Ok(nodes) => return Ok(nodes),
        Err(_) => {}
    }

    // Fall back to --nodelist.
    if let Some(raw) = &cli.nodelist {
        info!("Using explicit --nodelist.");
        return Ok(expand_nodelist(raw)?)
    }

    // Neither available.
    eprintln!(
        "error: Not inside a SLURM allocation and --nodelist was not provided.\n\
         Provide --nodelist <HOSTNAMES> or run inside a SLURM job."
    );
    std::process::exit(1);
}
