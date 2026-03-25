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
    parsers::{
        slurm::{expand_nodelist, get_nodelist_from_env, NodeListParseError},
        toml::{self, TomlTopologyOptions},
    },
    topology::{
        alps::get_groups_from_topo,
        jupiter::{self, JupiterOptions},
        leonardo::{self as leo, LeonardoOptions},
        NodeFilterOptions, SinfoSource, TopoSource,
    },
};

const SUPPORTED_SYSTEMS: &[&str] = &["leonardo", "jupiter", "alps"];

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Place jobs on the cluster topology according to a set of constraints.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Path to the query JSON file.  If omitted, the query is read from stdin.
    pub query: Option<PathBuf>,

    /// Explicit node list (SLURM nodes format).
    #[arg(short = 'n', long, value_name = "HOSTNAMES")]
    pub nodelist: Option<String>,

    /// Explicit node blacklist (SLURM nodes format).
    #[arg(short = 'b', long, value_name = "HOSTNAMES_BLACKLIST")]
    pub nodes_blacklist: Option<String>,

    /// Consider all available nodes.
    #[arg(short = 'a', long, conflicts_with = "nodelist")]
    pub all_nodes: bool,

    /// Enable informational/debug output.
    #[arg(short = 'v', long)]
    pub verbose: bool,

    /// System name
    /// Currently supported: leonardo, jupiter, alps FIXME use SUPPORTED_SYSTEMS
    #[arg(short = 's', long, value_name = "SYSTEM", value_parser = ["leonardo", "jupiter", "alps"])]
    pub system: String,

    /// System-specific topology source.
    #[command(flatten)]
    pub topo_source: TopoArgs,

    /// Read `sinfo` output from a file instead of running the command.
    #[arg(long, value_name = "PATH")]
    pub sinfo_file: Option<PathBuf>,

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
#[group(id = "topo_args", multiple = true)]
pub struct TopoArgs {
    /// Path to the system-specific topology file.
    #[arg(short = 'F', long, value_name = "PATH")]
    pub topology_file: Option<PathBuf>,

    #[arg(short = 'f', long, value_name = "PATH")]
    pub topology_toml_file: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

pub fn init_logger(verbose: bool) {
    let level = if verbose {
        LevelFilter::Info
    } else {
        LevelFilter::Error
    };
    env_logger::Builder::new().filter_level(level).init();
}

// ---------------------------------------------------------------------------
// Topology loading
// ---------------------------------------------------------------------------

/// Load, enrich, and filter the topology IR according to the CLI flags.
pub fn load_topology(cli: &Cli) -> Result<TopologyIR, Box<dyn std::error::Error>> {
    let system = &cli.system;
    let nodes_blacklist = match cli.nodes_blacklist.clone() {
        None => None,
        Some(bl) => Some(expand_nodelist(bl.as_str()).map_err(|e| {
            NodeListParseError::new(format!(
                "failed to parse nodes blacklist arg: {}. error: {}",
                bl.as_str(),
                e
            ))
        })?),
    };

    // Build NodeFilterOptions from flags.
    let filter_opts = NodeFilterOptions {
        remove_unavailable: !cli.include_unavailable,
        nodes_blacklist: nodes_blacklist.clone(),
    };

    // Resolve sinfo source (None means "no enrichment").
    let sinfo_source = resolve_sinfo_source(&cli);
    let topo_source = resolve_topo_source(&cli.topo_source);

    // Handle ALPS separately — it only supports TOML files.
    let mut ir = if cli.system == "alps" {
        match &topo_source {
            TopoSource::Command => {
                eprintln!("error: System ALPS requires a TOML file (you can find it in systems/).");
                std::process::exit(1);
            }
            TopoSource::Files(_f, t) => {
                if let Some(toml) = t {
                    info!("Loading topology from TOML: {}", toml.display());
                    let ir = toml::from_file_with_opts(
                        toml,
                        Some(sinfo_source),
                        filter_opts,
                        TomlTopologyOptions::default(),
                    )?;
                    match system.to_lowercase().as_str() {
                        "alps" => get_groups_from_topo(ir, topo_source)?,
                        _ => ir,
                    }
                } else {
                    eprintln!(
                        "error: System ALPS requires a TOML file (you can find it in systems/)."
                    );
                    std::process::exit(1);
                }
            }
        }
    } else {
        match topo_source {
            TopoSource::Command => {
                info!(
                    "Loading {} topology via `scontrol show topology`…",
                    system.to_uppercase()
                );
                match system.to_lowercase().as_str() {
                    "leonardo" => leo::from_scontrol_with_opts(
                        Some(sinfo_source),
                        filter_opts,
                        LeonardoOptions::default(),
                    )?,
                    "jupiter" => jupiter::from_scontrol_with_opts(
                        Some(sinfo_source),
                        filter_opts,
                        JupiterOptions::default(),
                    )?,
                    other => unknown_system(other),
                }
            }
            TopoSource::Files(_f, t) => {
                if let Some(file) = &cli.topo_source.topology_file {
                    info!(
                        "Loading {} topology from {}",
                        system.to_uppercase(),
                        file.display()
                    );
                    match system.to_lowercase().as_str() {
                        "leonardo" => leo::from_file_with_opts(
                            file,
                            Some(sinfo_source),
                            filter_opts,
                            LeonardoOptions::default(),
                        )?,
                        "jupiter" => jupiter::from_file_with_opts(
                            file,
                            Some(sinfo_source),
                            filter_opts,
                            JupiterOptions::default(),
                        )?,
                        other => unknown_system(other),
                    }
                } else if let Some(toml) = t {
                    parsers::toml::from_file(toml)? // fix 2: added `?`
                } else {
                    return Err("error: should not happen.".into()); // fix 3: `.into()` for Box<dyn Error>
                }
            }
        }
    };

    // Partition filter (applied after sinfo enrichment).
    if let Some(partition) = &cli.partition {
        info!("Filtering by partition: {}", partition);
        ir = filter_by_partition(ir, partition);
    }

    // Blacklist filter (applied after sinfo enrichment).
    if let Some(blacklist) = &nodes_blacklist {
        info!("Filtering by blacklist: {:?}", &blacklist);
        ir = filter_by_blacklist(ir, blacklist);
    }

    Ok(ir) // fix 5: idiomatic bare Ok(...)
}

// ---------------------------------------------------------------------------
// Node-list resolution
// ---------------------------------------------------------------------------

pub fn resolve_nodes_filter(cli: &Cli) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    if let Some(raw) = &cli.nodelist {
        info!("Using explicit --nodelist.");
        return Ok(expand_nodelist(raw)?);
    }
    match get_nodelist_from_env() {
        Ok(nodes) => return Ok(nodes),
        Err(_) => {}
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

fn resolve_sinfo_source(args: &Cli) -> SinfoSource {
    if let Some(f) = args.sinfo_file.clone() {
        if !f.exists() {
            eprintln!(
                "error: Sinfo file '{}' does not exist!",
                f.as_path().display()
            );
            std::process::exit(4);
        }
        SinfoSource::File(f)
    } else {
        SinfoSource::Command
    }
}

fn resolve_topo_source(args: &TopoArgs) -> TopoSource {
    if let Some(f) = args.topology_file.clone() {
        if !f.exists() {
            eprintln!(
                "error: Topology file '{}' does not exist!",
                f.as_path().display()
            );
            std::process::exit(5);
        }
        if let Some(toml) = args.topology_toml_file.clone() {
            if !toml.exists() {
                eprintln!(
                    "error: Topology TOML file '{}' does not exist!",
                    toml.as_path().display()
                );
                std::process::exit(6);
            }
            return TopoSource::Files(Some(f), Some(toml));
        } else {
            return TopoSource::Files(Some(f), None);
        }
    } else if let Some(toml) = args.topology_toml_file.clone() {
        if !toml.exists() {
            eprintln!(
                "error: Topology TOML file '{}' does not exist!",
                toml.as_path().display()
            );
            std::process::exit(7);
        }
        return TopoSource::Files(None, Some(toml));
    } else {
        return TopoSource::Command;
    }
}

/// Keep only compute nodes (and their topology path) that belong to
/// `partition`.  Switches are always kept as long as they have at least one
/// qualifying descendant.
fn filter_by_partition(ir: TopologyIR, partition: &str) -> TopologyIR {
    use crate::ir::EntityKind;

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

fn filter_by_blacklist(ir: TopologyIR, blacklist: &Vec<String>) -> TopologyIR {
    use crate::ir::EntityKind;

    // Collect IDs of compute nodes NOT in the requested partition.
    let to_remove: Vec<_> = ir
        .entities
        .values()
        .filter(|e| {
            if !matches!(e.kind, EntityKind::Compute) {
                return false;
            }
            blacklist.contains(&e.id.0)
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
    std::process::exit(3);
}
