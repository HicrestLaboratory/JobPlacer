pub mod graph;
pub mod ir;
pub mod parsers;
pub mod placement;
pub mod query;
pub mod topology;

use clap::clap_derive::Parser;
use clap::Args;
use log::{info, LevelFilter};
use std::path::PathBuf;

use crate::{
    ir::topology_ir::TopologyIR,
    parsers::slurm::{expand_nodelist, get_nodelist_from_env},
    topology::{
        NodeFilterOptions, SinfoSource, jupiter::{self, JupiterOptions}, leonardo::{self as leo, LeonardoOptions}
    },
};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Place jobs on the cluster topology according to a set of constraints.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Path to the query JSON file.  If omitted, the query is read from stdin.
    pub query: Option<PathBuf>,

    /// Explicit node list (comma- or newline-separated hostnames).
    /// Required when not running inside a SLURM allocation.
    #[arg(short = 'n', long, value_name = "HOSTNAMES")]
    pub nodelist: Option<String>,

    /// Consider all available nodes.
    #[arg(short = 'a', long)]
    pub all_nodes: bool,

    /// Enable informational/debug output.
    #[arg(short = 'v', long)]
    pub verbose: bool,

    /// How to load the system topology.
    #[command(flatten)]
    pub topology: TopologyArgs,

    /// System-specific topology source (used together with --system).
    #[command(flatten)]
    pub topo_source: TopoSource,

    /// sinfo data source for partition / state enrichment.
    #[command(flatten)]
    pub sinfo: SinfoArgs,

    /// Keep only nodes belonging to this partition (e.g. boost_usr_prod).
    /// If omitted, nodes from all partitions are kept.
    #[arg(short = 'p', long, value_name = "PARTITION")]
    pub partition: Option<String>,

    /// Include draining / drained / down nodes instead of filtering them out.
    #[arg(long)]
    pub include_unavailable: bool,

    /// Seed for placement RNG
    #[arg(
        long,
        help = "RNG seed for placement (different seeds yield different placements)"
    )]
    pub seed: Option<u64>,

    /// Enable graphical visualization
    #[arg(short = 'z', long)]
    pub visualize: bool,

    /// File to write the SVG to
    #[arg(long)]
    pub out_svg: Option<String>,
}

// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct TopologyArgs {
    /// Path to a YAML topology file (parsed with `manual::from_file`).
    #[arg(short = 'f', long, value_name = "PATH", group = "topo")]
    pub topology_yaml: Option<PathBuf>,

    /// Use a named system together with --topology-system-file or
    /// --topology-scontrol to specify the data source.
    /// Currently supported: leonardo, jupiter
    #[arg(short = 's', long, value_name = "SYSTEM", requires = "topo_source")]
    pub system: Option<String>,
}

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

/// Where to get `sinfo` partition / state data.
/// At most one of these may be set.
#[derive(Args, Debug)]
#[group(id = "sinfo_source", multiple = false)]
pub struct SinfoArgs {
    /// Run `sinfo` live to get partition and node-state information.
    #[arg(long)]
    pub sinfo: bool,

    /// Read `sinfo` output from a file instead of running the command.
    #[arg(long, value_name = "PATH", conflicts_with = "sinfo")]
    pub sinfo_file: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

pub fn init_logger(verbose: bool) {
    let level = if verbose {
        LevelFilter::Info
    } else {
        LevelFilter::Warn
    };
    env_logger::Builder::new().filter_level(level).init();
}

// ---------------------------------------------------------------------------
// Topology loading
// ---------------------------------------------------------------------------

const SUPPORTED_SYSTEMS: &[&str] = &["leonardo", "jupiter"];

/// Load, enrich, and filter the topology IR according to the CLI flags.
pub fn load_topology(cli: &Cli) -> Result<TopologyIR, Box<dyn std::error::Error>> {
    let topo = &cli.topology;

    // Build NodeFilterOptions from flags.
    let filter_opts = NodeFilterOptions {
        remove_unavailable: !cli.include_unavailable,
    };

    // Resolve sinfo source (None means "no enrichment").
    let sinfo_source = resolve_sinfo_source(&cli.sinfo);

    // ---- Option A: plain YAML file ----------------------------------
    if let Some(path) = &topo.topology_yaml {
        info!("Loading topology from YAML: {}", path.display());
        // YAML / manual parser is system-agnostic and doesn't use sinfo.
        use crate::parsers::manual;
        let ir = manual::from_file(path);
        return Ok(ir);
    }

    // ---- Option B / C: named system + source ------------------------
    if let Some(system) = &topo.system {
        let mut ir = if cli.topo_source.topology_scontrol {
            info!("Loading {} topology via scontrol…", system.to_uppercase());
            match system.to_lowercase().as_str() {
                "leonardo" => leo::from_scontrol_with_opts(
                    sinfo_source,
                    filter_opts,
                    LeonardoOptions::default(),
                )?,
                "jupiter" => jupiter::from_scontrol_with_opts(sinfo_source, filter_opts, JupiterOptions::default())?, // extend similarly when ready
                other => unknown_system(other),
            }
        } else if let Some(path) = &cli.topo_source.topology_file {
            info!(
                "Loading {} topology from {}",
                system.to_uppercase(),
                path.display()
            );
            match system.to_lowercase().as_str() {
                "leonardo" => leo::from_file_with_opts(
                    path,
                    sinfo_source,
                    filter_opts,
                    LeonardoOptions::default(),
                )?,
                "jupiter" => jupiter::from_file(path)?,
                other => unknown_system(other),
            }
        } else {
            eprintln!("error: required either --topology-file <PATH> or --topology-scontrol.");
            std::process::exit(1);
        };

        // Partition filter (applied after sinfo enrichment).
        if let Some(partition) = &cli.partition {
            ir = filter_by_partition(ir, partition);
        }

        return Ok(ir);
    }

    unreachable!("clap's required group guarantees one topology flag is set");
}

// ---------------------------------------------------------------------------
// Node-list resolution
// ---------------------------------------------------------------------------

pub fn resolve_nodes_filter(cli: &Cli) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    match get_nodelist_from_env() {
        Ok(nodes) => return Ok(nodes),
        Err(_) => {}
    }
    if let Some(raw) = &cli.nodelist {
        info!("Using explicit --nodelist.");
        return Ok(expand_nodelist(raw)?);
    }
    eprintln!(
        "error: Not inside a SLURM allocation and --nodelist was not provided.\n\
         Provide --nodelist <HOSTNAMES> or run inside a SLURM job."
    );
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn resolve_sinfo_source(args: &SinfoArgs) -> Option<SinfoSource> {
    if args.sinfo {
        Some(SinfoSource::Command)
    } else {
        args.sinfo_file.clone().map(SinfoSource::File)
    }
}

/// Keep only compute nodes (and their topology path) that belong to
/// `partition`.  Switches are always kept as long as they have at least one
/// qualifying descendant.
fn filter_by_partition(ir: TopologyIR, partition: &str) -> TopologyIR {
    use crate::ir::entity::EntityKind;

    // Collect IDs of compute nodes NOT in the requested partition.
    let to_remove: Vec<_> = ir
        .entities
        .values()
        .filter(|e| {
            if !matches!(e.kind, EntityKind::Compute) {
                return false;
            }
            !e.meta
                .get("partition")
                .map(|p| p.split(',').any(|part| part == partition))
                .unwrap_or(false)
        })
        .map(|e| e.id.clone())
        .collect();

    ir.filter_remove_ids(&to_remove)
}

fn unknown_system(name: &str) -> ! {
    eprintln!(
        "error: Unknown system '{}'. Supported: {}.",
        name,
        SUPPORTED_SYSTEMS.join(", ")
    );
    std::process::exit(1);
}
