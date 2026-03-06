use serde::Deserialize;

use crate::query::{Constraint, DistanceGroup, ReferencePoint, TopologyQuery};
use std::{fs, path::Path, str::FromStr};

#[derive(Deserialize, Debug)]
struct QueryInput {
    constraints: Vec<ConstraintInput>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum ConstraintInput {
    NodesAtDistance {
        count: usize,
        distance: i32,
        reference: String,
    },
    NodesAtDistanceWithSharedParent {
        count: usize,
        distance: i32,
        reference: String,
        parent_level: usize,
    },
    DistanceGroup {
        reference: String,
        groups: Vec<DistanceGroupInput>,
    },
}

#[derive(Deserialize, Debug)]
struct DistanceGroupInput {
    count: usize,
    distance: i32,
}

impl TopologyQuery {
    fn parse_reference_point(s: &str) -> ReferencePoint {
        match s.to_lowercase().as_str() {
            _ => ReferencePoint::First,
        }
    }
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        content.parse()
    }
}

impl FromStr for TopologyQuery {
    type Err = Box<dyn std::error::Error>;

    fn from_str(json_content: &str) -> Result<Self, Self::Err> {
        let query_input: QueryInput = serde_json::from_str(json_content)?;

        let mut query = TopologyQuery::new();

        for c in query_input.constraints {
            let constraint = match c {
                ConstraintInput::NodesAtDistance {
                    count,
                    distance,
                    reference,
                } => Constraint::NodesAtDistance {
                    count,
                    distance: distance as f32,
                    reference: Self::parse_reference_point(&reference),
                },

                ConstraintInput::NodesAtDistanceWithSharedParent {
                    count,
                    distance,
                    reference,
                    parent_level,
                } => Constraint::NodesAtDistanceWithSharedParent {
                    count,
                    distance: distance as f32,
                    reference: Self::parse_reference_point(&reference),
                    parent_level,
                },

                ConstraintInput::DistanceGroup { reference, groups } => {
                    let parsed_groups = groups
                        .into_iter()
                        .map(|g| DistanceGroup {
                            count: g.count,
                            distance: g.distance as f32,
                        })
                        .collect();

                    Constraint::DistanceGroup {
                        reference: Self::parse_reference_point(&reference),
                        groups: parsed_groups,
                    }
                }
            };

            query = query.with_constraint(constraint);
        }

        Ok(query)
    }
}
