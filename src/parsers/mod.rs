use std::{fs, path::Path, process::Command};

use crate::parsers::slurm::NodeListParseError;

pub mod toml;

pub mod sinfo;
pub mod slurm;

pub fn topology_from_file_raw<P: AsRef<Path>>(path: P) -> Result<String, NodeListParseError> {
    fs::read_to_string(path)
        .map_err(|e| NodeListParseError::new(format!("failed to read sinfo file: {e}")))
}

pub fn run_scontrol_show_topology() -> String {
    let output = Command::new("scontrol")
        .arg("-d")
        .arg("show")
        .arg("topology")
        .output()
        .expect("Failed to execute scontrol show topology");

    if !output.status.success() {
        panic!(
            "scontrol command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    return String::from_utf8(output.stdout).expect("Invalid UTF-8 in scontrol output");
}
