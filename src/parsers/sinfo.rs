use std::{collections::HashMap, fs, path::Path, process::Command};

use crate::parsers::slurm::{expand_nodelist, NodeListParseError};

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
            "mix" | "mixed" => NodeState::Mixed,
            "idle" => NodeState::Idle,
            "drng" | "draining" => NodeState::Draining,
            "drain" | "drained" => NodeState::Drained,
            "down" => NodeState::Down,
            other => NodeState::Other(other.to_string()),
        }
    }

    /// Returns `true` for states that indicate the node is not usable for new jobs.
    pub fn is_unavailable(&self) -> bool {
        matches!(
            self,
            NodeState::Draining | NodeState::Drained | NodeState::Down
        )
    }
}

// ---------------------------------------------------------------------------
// Per-node sinfo record
// ---------------------------------------------------------------------------

/// Everything sinfo tells us about a single compute node.
#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub hostname: String,
    pub partition: String,
    pub state: NodeState,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

pub fn from_sinfo_command_raw() -> Result<String, NodeListParseError> {
    let output = Command::new("sinfo")
        .args(["-h", "-N", "-o", "%N %P %t"])
        .output()
        .map_err(|e| NodeListParseError::new(format!("failed to run sinfo: {e}")))?;

    String::from_utf8(output.stdout)
        .map_err(|_| NodeListParseError::new("sinfo output is not valid UTF-8"))
}

pub fn from_sinfo_file_raw<P: AsRef<Path>>(path: P) -> Result<String, NodeListParseError> {
    fs::read_to_string(path)
        .map_err(|e| NodeListParseError::new(format!("failed to read sinfo file: {e}")))
}

/// Run `sinfo -h -N -o "%N %P %t"` and parse the output.
///
/// `-h`  suppress header
/// `-N`  one line per node
/// `-o`  custom format: nodename  partition  state
pub fn from_sinfo_command() -> Result<Vec<NodeInfo>, NodeListParseError> {
    parse_sinfo_output(&from_sinfo_command_raw()?)
}

/// Parse sinfo output from a file (useful for testing / offline use).
pub fn from_sinfo_file<P: AsRef<Path>>(path: P) -> Result<Vec<NodeInfo>, NodeListParseError> {
    parse_sinfo_output(&from_sinfo_file_raw(path)?)
}

/// Parse the raw sinfo text.
///
/// Supports two input formats, auto-detected:
///
/// **Format 1** — `sinfo -h -N -o "%N %P %t"` (no header, 3 columns):
/// ```text
/// nid005758 normal* plnd
/// nid005759 low     alloc
/// ```
///
/// **Format 2** — default `sinfo` table output (header present, variable columns):
/// ```text
/// PARTITION AVAIL JOB_SIZE TIMELIMIT CPUS S:C:T NODES STATE NODELIST
/// debug     up    1-10     30:00     288  4:72:1 3     alloc nid[006544-006545]
/// ```
/// Column discovery is header-driven, so it is robust to column reordering,
/// added columns, or missing optional columns.
pub fn parse_sinfo_output(raw: &str) -> Result<Vec<NodeInfo>, NodeListParseError> {
    // Collect non-empty lines, skipping blanks.
    let lines: Vec<&str> = raw.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    // Detect format: if any line starts with "PARTITION" (case-insensitive) treat
    // the whole input as a header-driven table (Format 2).
    let header_line_idx = lines
        .iter()
        .position(|l| l.to_uppercase().starts_with("PARTITION"));

    match header_line_idx {
        Some(idx) => parse_header_format(&lines[idx..]),
        None => parse_fixed_format(&lines),
    }
}

// ---------------------------------------------------------------------------
// Format 1: fixed 3-column "%N %P %t" output (no header)
// ---------------------------------------------------------------------------

fn parse_fixed_format(lines: &[&str]) -> Result<Vec<NodeInfo>, NodeListParseError> {
    let mut records = Vec::new();

    for line in lines {
        let cols: Vec<&str> = line.splitn(3, char::is_whitespace).collect();
        if cols.len() < 3 {
            continue;
        }
        let (nodelist_str, partition, state_str) = (cols[0], cols[1], cols[2]);

        let partition = partition.trim_end_matches('*');
        let state = NodeState::from_str(state_str.trim());

        if nodelist_str == "N/A" || nodelist_str.is_empty() {
            continue;
        }

        for hostname in expand_nodelist(nodelist_str)? {
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
// Format 2: header-driven table (default sinfo output)
// ---------------------------------------------------------------------------

/// Required columns (case-insensitive).  NODELIST is matched by prefix
/// because it sometimes appears as "NODELIST(REASON)" etc.
const COL_PARTITION: &str = "PARTITION";
const COL_STATE: &str = "STATE";
const COL_NODELIST: &str = "NODELIST";

fn parse_header_format(lines: &[&str]) -> Result<Vec<NodeInfo>, NodeListParseError> {
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    // --- Locate required columns from the header ----------------------------
    let header = lines[0];
    let col_spans = column_spans(header);

    let find_col = |needle: &str| -> Option<usize> {
        col_spans.iter().position(|(name, _)| {
            name.to_uppercase().starts_with(needle)
        })
    };

    let part_col = find_col(COL_PARTITION).ok_or_else(|| {
        NodeListParseError::new("sinfo table header missing PARTITION column")
    })?;
    let state_col = find_col(COL_STATE).ok_or_else(|| {
        NodeListParseError::new("sinfo table header missing STATE column")
    })?;
    let node_col = find_col(COL_NODELIST).ok_or_else(|| {
        NodeListParseError::new("sinfo table header missing NODELIST column")
    })?;

    // --- Parse data rows -----------------------------------------------------
    let mut records = Vec::new();

    for line in &lines[1..] {
        // The NODELIST column is always last and may itself contain spaces in
        // some edge cases; split only up to the number of header columns so the
        // tail is kept intact.
        let values = split_row(line, &col_spans);
        if values.len() <= node_col.max(state_col).max(part_col) {
            continue; // malformed / short row
        }

        let partition = values[part_col].trim_end_matches('*');
        let state = NodeState::from_str(values[state_col].trim());
        let nodelist_str = values[node_col].trim();

        if nodelist_str.is_empty() || nodelist_str == "N/A" {
            continue;
        }

        for hostname in expand_nodelist(nodelist_str)? {
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
// Table layout helpers
// ---------------------------------------------------------------------------

/// Returns `(column_name, start_byte_offset)` for every whitespace-separated
/// token in the header line, preserving insertion order.
fn column_spans(header: &str) -> Vec<(String, usize)> {
    let mut spans = Vec::new();
    let mut in_word = false;
    let mut word_start = 0;

    for (i, ch) in header.char_indices() {
        match (in_word, ch.is_whitespace()) {
            (false, false) => { in_word = true; word_start = i; }
            (true, true) => {
                spans.push((header[word_start..i].to_string(), word_start));
                in_word = false;
            }
            _ => {}
        }
    }
    if in_word {
        spans.push((header[word_start..].to_string(), word_start));
    }
    spans
}

/// Split a data row into fields aligned to the byte offsets from the header.
///
/// Each field runs from its column's start offset to the next column's start
/// offset (or end of string), then gets trimmed.  This handles fixed-width
/// sinfo output where values may be shorter than their column but are always
/// left-aligned under their header token.
///
/// Falls back to plain whitespace splitting when the line is shorter than
/// expected (e.g. truncated rows).
fn split_row<'a>(line: &'a str, spans: &[(String, usize)]) -> Vec<&'a str> {
    let bytes = line.as_bytes();
    let mut fields = Vec::with_capacity(spans.len());

    for (i, (_, start)) in spans.iter().enumerate() {
        let start = (*start).min(bytes.len());
        let end = spans
            .get(i + 1)
            .map(|(_, s)| (*s).min(bytes.len()))
            .unwrap_or(bytes.len());

        // Ensure we slice on a valid char boundary.
        let start = nearest_char_boundary(line, start);
        let end = nearest_char_boundary(line, end);

        fields.push(line[start..end].trim());
    }

    fields
}

/// Round `pos` down to the nearest valid UTF-8 char boundary in `s`.
fn nearest_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut p = pos;
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Build a map  hostname → Vec<NodeInfo>  (a node can appear in multiple partitions).
pub fn index_by_hostname(infos: Vec<NodeInfo>) -> HashMap<String, Vec<NodeInfo>> {
    let mut map: HashMap<String, Vec<NodeInfo>> = HashMap::new();
    for info in infos {
        map.entry(info.hostname.clone()).or_default().push(info);
    }
    map
}