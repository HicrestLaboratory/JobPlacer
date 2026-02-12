// lib.rs
pub mod builder;
pub mod ir;
pub mod parsers;
pub mod query;

#[cfg(feature = "python")]
pub mod python;

#[cfg(feature = "python")]
pub use python::TopologyQueryBuilder; // re-export here

#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "python")]
#[pymodule]
fn job_placer(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // add_class is now available on the Bound<'_, PyModule> type
    m.add_class::<TopologyQueryBuilder>()?; 
    Ok(())
}