use std::collections::HashMap;
use std::fmt;
use std::process::Command;
use log::info;

pub fn get_nodelist_from_env() -> Result<Vec<String>, String> {
    if let Ok(nodelist_env) = std::env::var("SLURM_JOB_NODELIST") {
        info!("Detected SLURM environment, expanding node list…");
        let output = Command::new("scontrol")
            .args(["show", "hostnames", &nodelist_env])
            .output().map_err(|err| err.to_string())?;
        let stdout = String::from_utf8(output.stdout).map_err(|_| String::from("Could not parse stdout to utf8"))?;
        let nodes: Vec<String> = stdout
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !nodes.is_empty() {
            return Ok(nodes);
        }
    }
    Err(String::from("Env variable SLURM_JOB_NODELIST is not set."))
}

#[derive(Debug)]
pub struct NodeListParseError {
    msg: String,
}

impl NodeListParseError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
}

impl fmt::Display for NodeListParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid SLURM nodelist: {}", self.msg)
    }
}

impl std::error::Error for NodeListParseError {}


/// Expand SLURM-style node list expressions
pub fn expand_nodelist(input: &str) -> Result<Vec<String>, NodeListParseError> {
    let main = input.split_whitespace().next().unwrap_or(input);

    let items = split_preserving_brackets(main)?;

    let mut result = Vec::new();

    for item in items {
        result.extend(expand_item(&item)?);
    }

    Ok(result)
}

/// Expand one item that may contain multiple bracket groups
fn expand_item(s: &str) -> Result<Vec<String>, NodeListParseError> {
    if let Some(start) = s.find('[') {
        let end = s[start..]
            .find(']')
            .map(|e| e + start)
            .ok_or_else(|| NodeListParseError::new(format!("missing closing ']' in '{}'", s)))?;

        let prefix = &s[..start];
        let inside = &s[start + 1..end];
        let suffix = &s[end + 1..];

        if inside.is_empty() {
            return Err(NodeListParseError::new(format!(
                "empty bracket expression in '{}'",
                s
            )));
        }

        let mut results = Vec::new();

        for part in inside.split(',') {
            let expanded = expand_part(part)?;

            for val in expanded {
                let new_string = format!("{}{}{}", prefix, val, suffix);
                results.extend(expand_item(&new_string)?);
            }
        }

        Ok(results)
    } else {
        Ok(vec![s.to_string()])
    }
}

/// Expand a single range element (e.g. 01-04 or 05)
fn expand_part(part: &str) -> Result<Vec<String>, NodeListParseError> {
    if let Some(dash) = part.find('-') {
        let start_str = &part[..dash];
        let end_str = &part[dash + 1..];

        if start_str.is_empty() || end_str.is_empty() {
            return Err(NodeListParseError::new(format!(
                "invalid range '{}'",
                part
            )));
        }

        let start: usize = start_str
            .parse()
            .map_err(|_| NodeListParseError::new(format!("invalid number '{}'", start_str)))?;

        let end: usize = end_str
            .parse()
            .map_err(|_| NodeListParseError::new(format!("invalid number '{}'", end_str)))?;

        if start > end {
            return Err(NodeListParseError::new(format!(
                "range start greater than end in '{}'",
                part
            )));
        }

        let width = start_str.len();

        Ok((start..=end)
            .map(|v| format!("{:0width$}", v, width = width))
            .collect())
    } else {
        if !part.chars().all(|c| c.is_ascii_digit()) {
            return Err(NodeListParseError::new(format!(
                "invalid element '{}'",
                part
            )));
        }

        Ok(vec![part.to_string()])
    }
}

/// Split comma-separated items but keep bracket groups intact
fn split_preserving_brackets(s: &str) -> Result<Vec<String>, NodeListParseError> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    for ch in s.chars() {
        match ch {
            '[' => {
                depth += 1;
                current.push(ch);
            }
            ']' => {
                if depth == 0 {
                    return Err(NodeListParseError::new(
                        "closing ']' without matching '['",
                    ));
                }
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                if !current.is_empty() {
                    result.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }

    if depth != 0 {
        return Err(NodeListParseError::new("missing closing ']'"));
    }

    if !current.is_empty() {
        result.push(current);
    }

    Ok(result)
}

/// Parse line into key-value map
pub fn parse_line(line: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in line.split_whitespace() {
        if let Some(pos) = part.find('=') {
            let key = part[..pos].to_string();
            let val = part[pos+1..].to_string();
            map.insert(key, val);
        }
    }
    map
}
