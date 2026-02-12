use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyDictMethods}; // Import PyDictMethods trait for set_item
use std::collections::HashMap;
use pyo3::types::PyAny; 

use crate::ir::id::Id;
use crate::ir::topology_ir::TopologyIR;
use crate::query::{TopologyQuery, Constraint, ReferencePoint, DistanceGroup, DistanceGroupWithParent};
use crate::builder::graph::graph_from_ir;
use crate::builder::display::display_graph;

use petgraph::visit::EdgeRef;

/// Python bindings for TopologyQueryBuilder
#[pyclass]
#[derive(Clone)]
pub struct TopologyQueryBuilder {
    ir: TopologyIR,
}

#[pymethods]
impl TopologyQueryBuilder {
    #[new]
    fn new(parser: String, path: Option<String>) -> PyResult<Self> {
        match (parser.as_str(), path) {
            ("leonardo", None) => Ok(Self { ir: crate::parsers::leonardo::from_scontrol() }),
            ("leonardo", Some(p)) => Ok(Self { ir: crate::parsers::leonardo::from_file(p) }),
            ("manual", Some(p)) => Ok(Self { ir: crate::parsers::manual::from_file(p) }),
            _ => Err(PyRuntimeError::new_err("Invalid parser/path combination")),
        }
    }

    /// This bridges the Builder to the Interface
    fn get_interface(&self) -> TopologyInterface {
        TopologyInterface { 
            ir: self.ir.clone() 
        }
    }

    fn get_compute_nodes(&self) -> Vec<String> {
        use crate::ir::entity::EntityKind;
        self.ir.entities.iter()
            .filter(|(_, e)| matches!(e.kind, EntityKind::Compute))
            .map(|(id, _)| id.0.clone())
            .collect()
    }

    fn is_valid_compute_node(&self, node_id: String) -> bool {
        use crate::ir::entity::EntityKind;
        let id = Id::from(node_id.as_str());
        self.ir.entities.get(&id)
            .map(|e| matches!(e.kind, EntityKind::Compute))
            .unwrap_or(false)
    }

    fn filter_by_ids(&mut self, ids: Vec<String>) {
        //use filter with topology to keep only specified IDs and their relationships
        let id_set: Vec<Id> = ids.into_iter().map(|s| Id::from(s.as_str())).collect();
        self.ir = self.ir.filter_with_topology(&id_set);
    }

    fn get_nodelist_distances(&self, anchor: String, distances: Vec<(usize, f32)>) -> PyResult<Vec<String>> {
        let anchor_id = Id::from(anchor.as_str());
        let groups: Vec<DistanceGroup> = distances.into_iter()
            .map(|(count, distance)| DistanceGroup { count, distance })
            .collect();

        let query = TopologyQuery::new().with_constraint(Constraint::DistanceGroup {
            reference: ReferencePoint::First,
            groups,
        });

        let selected = query.execute_from(&self.ir, anchor_id)
            .map_err(|e| PyRuntimeError::new_err(format!("{}", e)))?;

        Ok(selected.into_iter().map(|id| id.0).collect())
    }

    fn get_nodelist_distances_shared_parent(&self, anchor: String, distances: Vec<(usize, f32, usize)>) -> PyResult<Vec<String>> {
        let anchor_id = Id::from(anchor.as_str());
        let groups: Vec<DistanceGroupWithParent> = distances.into_iter()
            .map(|(count, distance, parent_level)| DistanceGroupWithParent { count, distance, parent_level })
            .collect();

        let query = TopologyQuery::new().with_constraint(Constraint::DistanceGroupWithSharedParent {
            reference: ReferencePoint::First,
            groups,
        });

        let selected = query.execute_from(&self.ir, anchor_id)
            .map_err(|e| PyRuntimeError::new_err(format!("{}", e)))?;

        Ok(selected.into_iter().map(|id| id.0).collect())
    }

    fn visualize_topology(&self, nodes: Vec<String>, output_file: String) -> PyResult<()> {
        let node_ids: Vec<Id> = nodes.iter().map(|s| Id::from(s.as_str())).collect();
        let filtered_ir = self.ir.filter_with_topology(&node_ids);

        let (graph, _) = graph_from_ir(&filtered_ir);
        display_graph(&graph, &filtered_ir, &output_file);

        Ok(())
    }
}

#[pyclass]
#[derive(Clone)]
pub struct TopologyInterface {
    pub ir: TopologyIR,
}

#[pymethods]
impl TopologyInterface {
    pub fn get_edges(&self) -> Vec<(String, String, f32)> {
        let (graph, _) = crate::builder::graph::graph_from_ir(&self.ir);
        graph.edge_references()
            .map(|e| (
                graph[e.source()].0.clone(),
                graph[e.target()].0.clone(),
                *e.weight()
            ))
            .collect()
    }

    /// Returns physical containment as a Python dict: { parent: [children] }
    pub fn get_hierarchy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (parent, children) in &self.ir.contains {
            let child_ids: Vec<String> = children.iter().map(|c| c.0.clone()).collect();
            // Use set_item from PyDictMethods trait
            dict.set_item(parent.0.clone(), child_ids)?;
        }
        Ok(dict)
    }

    /// Returns node attributes as a Python dict: { id: {"kind": str} }
    pub fn get_metadata<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let outer_dict = PyDict::new(py);
        for (id, entity) in &self.ir.entities {
            let inner_dict = PyDict::new(py);
            
            let kind_str = format!("{:?}", entity.kind);
            // In modern PyO3, set_item handles the conversion automatically
            inner_dict.set_item("kind", kind_str)?;
            
            outer_dict.set_item(id.0.clone(), inner_dict)?;
        }
        Ok(outer_dict)
    }
}