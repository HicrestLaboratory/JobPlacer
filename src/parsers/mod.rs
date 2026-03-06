use std::process::Command;

pub mod manual;
pub mod leonardo;
pub mod jupiter;

pub mod slurm;

fn run_scontrol_show_topology() -> String {
    let output = Command::new("scontrol")
        .arg("-d")
        .arg("show")
        .arg("topology")
        .output()
        .expect("Failed to execute scontrol show topology");

    if !output.status.success() {
        panic!("scontrol command failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    return String::from_utf8(output.stdout).expect("Invalid UTF-8 in scontrol output");
}