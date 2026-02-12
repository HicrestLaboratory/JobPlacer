// lib.rs
pub mod builder;
pub mod ir;
pub mod parsers;
pub mod query;

#[cfg(feature = "python")]
pub mod python;

#[cfg(feature = "python")]
pub use python::TopologyQueryBuilder; // re-export here
pub use python::TopologyInterface; // re-export here

#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "python")]
#[pymodule]
fn job_placer(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Export the Builder for creating queries
    m.add_class::<TopologyQueryBuilder>()?; 
    
    // Export the Interface for NetworkX visualization data
    m.add_class::<TopologyInterface>()?; 
    
    Ok(())
}