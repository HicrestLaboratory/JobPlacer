use std::{collections::HashMap, fs, path::Path, process::Command};

use crate::parsers::slurm::{NodeListParseError, expand_nodelist};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// State of a compute node as reported by sinfo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeState {
    Allocated,
    Mixed,
    Idle,
    /// Node is being drained (still running jobs but accepts no new ones).
    Draining,
    /// Node has been fully drained.
    Drained,
    Down,
    /// Any other state string we don't explicitly model.
    Other(String),
}

impl NodeState {
    pub fn from_str(s: &str) -> Self {
        // sinfo appends '*' (non-responding) or '~' (power-saving) suffixes;
        // strip them before matching.
        let clean = s.trim_end_matches(|c| c == '*' || c == '~' || c == '#' || c == '!');
        match clean {
            "alloc" | "allocated" => NodeState::Allocated,
            "mix"   | "mixed"     => NodeState::Mixed,
            "idle"                => NodeState::Idle,
            "drng"  | "draining"  => NodeState::Draining,
            "drain" | "drained"   => NodeState::Drained,
            "down"                => NodeState::Down,
            other                 => NodeState::Other(other.to_string()),
        }
    }

    /// Returns `true` for states that indicate the node is not usable for new jobs.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, NodeState::Draining | NodeState::Drained | NodeState::Down)
    }
}

// ---------------------------------------------------------------------------
// Per-node sinfo record
// ---------------------------------------------------------------------------

/// Everything sinfo tells us about a single compute node.
#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub hostname:  String,
    pub partition: String,
    pub state:     NodeState,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Run `sinfo -h -N -o "%N %P %t"` and parse the output.
///
/// `-h`  suppress header
/// `-N`  one line per node
/// `-o`  custom format: nodename  partition  state
pub fn from_sinfo_command() -> Result<Vec<NodeInfo>, NodeListParseError> {
    let output = Command::new("sinfo")
        .args(["-h", "-N", "-o", "%N %P %t"])
        .output()
        .map_err(|e| NodeListParseError::new(format!("failed to run sinfo: {e}")))?;

    let raw = String::from_utf8(output.stdout)
        .map_err(|_| NodeListParseError::new("sinfo output is not valid UTF-8"))?;

    parse_sinfo_output(&raw)
}

/// Parse sinfo output from a file (useful for testing / offline use).
pub fn from_sinfo_file<P: AsRef<Path>>(path: P) -> Result<Vec<NodeInfo>, NodeListParseError> {
    let raw = fs::read_to_string(path)
        .map_err(|e| NodeListParseError::new(format!("failed to read sinfo file: {e}")))?;
    parse_sinfo_output(&raw)
}

/// Parse the raw sinfo text.
///
/// We support two formats:
///
/// 1. **Node-per-line** (`sinfo -N -o "%N %P %t"`):
///    ```
///    lrdn0001  boost_usr_prod  mix
///    ```
///
/// 2. **Range-per-line** (default `sinfo` table with nodelist column), e.g.:
///    ```
///    boost_usr_prod  up  1-00:00:00  598  mix  lrdn[0001-0003,…]
///    ```
///    Column order: PARTITION AVAIL TIMELIMIT NODES STATE NODELIST
pub fn parse_sinfo_output(raw: &str) -> Result<Vec<NodeInfo>, NodeListParseError> {
    let mut records = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("PARTITION") {
            continue; // skip header or blank
        }

        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 {
            continue;
        }

        // Detect format by checking whether the 1st column looks like a hostname
        // (no '/' or time-limit pattern) vs a partition name.
        //
        // Node-per-line format: cols = [nodename, partition, state]
        // Table format:         cols = [partition, avail, timelimit, count, state, nodelist…]
        let (partition, state_str, nodelist_str) = if cols.len() == 3 {
            // Explicit "%N %P %t" format
            (cols[1], cols[2], cols[0])
        } else if cols.len() >= 6 {
            // Default sinfo table: partition avail timelimit count state nodelist
            (cols[0], cols[4], cols[5])
        } else {
            continue;
        };

        // Partition names sometimes end with '*' (default partition marker)
        let partition = partition.trim_end_matches('*');
        let state = NodeState::from_str(state_str);

        // nodelist_str may be a single hostname, a bracketed range, or "N/A"
        if nodelist_str == "N/A" || nodelist_str.is_empty() {
            continue;
        }

        let hostnames = expand_nodelist(nodelist_str)?;
        for hostname in hostnames {
            records.push(NodeInfo {
                hostname,
                partition: partition.to_string(),
                state: state.clone(),
            });
        }
    }

    Ok(records)
}

// ---------------------------------------------------------------------------
// Index helper
// ---------------------------------------------------------------------------

/// Build a map  hostname → Vec<NodeInfo>  (a node can appear in multiple partitions).
pub fn index_by_hostname(infos: Vec<NodeInfo>) -> HashMap<String, Vec<NodeInfo>> {
    let mut map: HashMap<String, Vec<NodeInfo>> = HashMap::new();
    for info in infos {
        map.entry(info.hostname.clone()).or_default().push(info);
    }
    map
}