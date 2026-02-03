use crate::ir::topology_ir::TopologyIR;

pub fn validate(ir: &TopologyIR) -> Result<(), String> {
    for link in &ir.links {
        if !ir.entities.contains_key(&link.from) {
            return Err(format!("Missing entity {:?}", link.from));
        }
        if !ir.entities.contains_key(&link.to) {
            return Err(format!("Missing entity {:?}", link.to));
        }
    }
    Ok(())
}
