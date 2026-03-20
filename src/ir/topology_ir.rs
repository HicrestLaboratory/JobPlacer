use crate::ir::Id;
use crate::ir::Link;
use crate::ir::{Entity, EntityKind};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

/// Intermediate representation of the systems topology
#[derive(Default, Debug, Clone)]
pub struct TopologyIR {
    pub entities: HashMap<Id, Entity>,
    pub links: Vec<Link>,
    pub contains: HashMap<Id, Vec<Id>>,
}

impl TopologyIR {
    pub fn add_entity(&mut self, entity: Entity) {
        self.entities.insert(entity.id.clone(), entity);
    }

    pub fn add_link(&mut self, from: Id, to: Id, weight: f32) {
        self.links.push(Link { from, to, weight });
    }

    pub fn add_contains(&mut self, parent: Id, child: Id) {
        self.contains.entry(parent).or_default().push(child);
    }

    pub fn remove_entity(&mut self, id: &Id) {
        self.entities.remove(id);
        // also clean up any contains/links referencing this id if your IR
        // stores those as indexed collections rather than deriving them
    }

    /// Filter the topology keeping only entities that match the predicate
    pub fn filter<F>(&self, predicate: F) -> TopologyIR
    where
        F: Fn(&Entity) -> bool,
    {
        let mut filtered = TopologyIR::default();

        // Collect IDs of entities that pass the filter
        let valid_ids: HashSet<Id> = self
            .entities
            .values()
            .filter(|entity| predicate(entity))
            .map(|entity| entity.id.clone())
            .collect();

        // Add filtered entities
        for id in &valid_ids {
            if let Some(entity) = self.entities.get(id) {
                filtered.add_entity(entity.clone());
            }
        }

        // Add links where both endpoints exist in filtered entities
        for link in &self.links {
            if valid_ids.contains(&link.from) && valid_ids.contains(&link.to) {
                filtered.add_link(link.from.clone(), link.to.clone(), link.weight);
            }
        }

        // Add containment relationships where both parent and child exist
        for (parent, children) in &self.contains {
            if valid_ids.contains(parent) {
                for child in children {
                    if valid_ids.contains(child) {
                        filtered.add_contains(parent.clone(), child.clone());
                    }
                }
            }
        }

        filtered
    }

    /// Filter keeping only specified IDs
    pub fn filter_by_ids(&self, ids: &[Id]) -> TopologyIR {
        let id_set: HashSet<Id> = ids.iter().cloned().collect();
        self.filter(|entity| id_set.contains(&entity.id))
    }

    /// Filter removing specified IDs
    pub fn filter_remove_ids(&self, ids: &[Id]) -> TopologyIR {
        let id_set: HashSet<Id> = ids.iter().cloned().collect();
        self.filter(|entity| !id_set.contains(&entity.id))
    }

    /// Filter keeping entities matching a set of conditions
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Keep only compute nodes
    /// let filtered = ir.filter(|e| e.node_type == "compute");
    ///
    /// // Keep nodes with high capacity
    /// let filtered = ir.filter(|e| e.capacity > 100);
    ///
    /// // Complex filter
    /// let filtered = ir.filter(|e| {
    ///     e.node_type == "compute" && e.zone == "us-west" && e.capacity > 50
    /// });
    /// ```
    pub fn filter_chain(&'_ self) -> FilterChain<'_> {
        FilterChain::new(self)
    }

    /// Filter compute nodes but keep their complete topology path
    /// This keeps the target nodes plus all their ancestors (switches, routers, etc.)
    pub fn filter_with_topology(&self, target_ids: &[Id]) -> TopologyIR {
        let mut nodes_to_keep: HashSet<Id> = target_ids.iter().cloned().collect();

        // Find all parents recursively
        let mut changed = true;
        while changed {
            changed = false;
            let current_nodes: Vec<Id> = nodes_to_keep.iter().cloned().collect();

            for (parent, children) in &self.contains {
                for child in children {
                    if current_nodes.contains(child) && !nodes_to_keep.contains(parent) {
                        nodes_to_keep.insert(parent.clone());
                        changed = true;
                    }
                }
            }
        }

        let keep_vec: Vec<Id> = nodes_to_keep.into_iter().collect();
        self.filter_by_ids(&keep_vec)
    }
}

impl fmt::Display for TopologyIR {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // --- Counts by kind ---
        let mut n_compute = 0usize;
        let mut n_l1 = 0usize;
        let mut n_l2 = 0usize;
        let mut n_other_switch = 0usize;
        let mut n_group = 0usize;

        for entity in self.entities.values() {
            match &entity.kind {
                EntityKind::Compute => n_compute += 1,
                EntityKind::Switch { level: Some(0) } => n_l1 += 1,
                EntityKind::Switch { level: Some(1) } => n_l2 += 1,
                EntityKind::Switch { .. } => n_other_switch += 1,
                EntityKind::Group => n_group += 1,
            }
        }

        // --- Header ---
        writeln!(
            f,
            "TopologyIR {{ entities: {}, links: {}, containment_edges: {} }}",
            self.entities.len(),
            self.links.len(),
            self.contains.values().map(|v| v.len()).sum::<usize>(),
        )?;

        // --- Entity breakdown ---
        writeln!(f, "  Entities:")?;
        writeln!(f, "    Compute       : {n_compute}")?;
        writeln!(f, "    L1 switches   : {n_l1}")?;
        writeln!(f, "    L2 switches   : {n_l2}")?;
        if n_other_switch > 0 {
            writeln!(f, "    Other switches: {n_other_switch}")?;
        }
        if n_group > 0 {
            writeln!(f, "    Groups        : {n_group}")?;
        }

        // --- Link weight statistics ---
        if !self.links.is_empty() {
            let (mut min_w, mut max_w, mut sum_w) = (f32::MAX, f32::MIN, 0f32);
            for link in &self.links {
                min_w = min_w.min(link.weight);
                max_w = max_w.max(link.weight);
                sum_w += link.weight;
            }
            let avg_w = sum_w / self.links.len() as f32;
            writeln!(f, "  Links: min={min_w:.2}  avg={avg_w:.2}  max={max_w:.2}")?;
        }

        // --- Cell breakdown ---
        // Reconstruct cells from switch metadata
        #[derive(Default)]
        struct CellStats {
            racks: BTreeSet<String>,
            n_l1: usize,
            n_l2: usize,
            n_compute: usize,
            partitions: BTreeSet<String>,
        }

        let mut cells: BTreeMap<String, CellStats> = BTreeMap::new();

        for entity in self.entities.values() {
            match &entity.kind {
                EntityKind::Switch { level: Some(0) } => {
                    if let Some(cell) = entity.meta.get("cell") {
                        let stats = cells.entry(cell.clone()).or_default();
                        stats.n_l1 += 1;
                        if let Some(rack) = entity.meta.get("rack") {
                            stats.racks.insert(rack.clone());
                        }
                    }
                }
                EntityKind::Switch { level: Some(1) } => {
                    if let Some(cell) = entity.meta.get("cell") {
                        cells.entry(cell.clone()).or_default().n_l2 += 1;
                    }
                }
                EntityKind::Compute => {
                    // Find which cell owns this node via containment:
                    // compute → L1 switch (has cell metadata)
                    let parent_cell = self
                        .contains
                        .iter()
                        .find(|(_, children)| children.contains(&entity.id))
                        .and_then(|(parent_id, _)| self.entities.get(parent_id))
                        .and_then(|parent| parent.meta.get("cell"))
                        .cloned();

                    if let Some(cell) = parent_cell {
                        let stats = cells.entry(cell).or_default();
                        stats.n_compute += 1;
                        if let Some(parts) = entity.meta.get("partitions") {
                            for p in parts.split(',') {
                                stats.partitions.insert(p.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if !cells.is_empty() {
            writeln!(f, "  Cells: {}", cells.len())?;
            for (cell_name, stats) in &cells {
                let partition_str = if stats.partitions.is_empty() {
                    String::from("—")
                } else {
                    stats
                        .partitions
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                writeln!(
                    f,
                    "    {cell_name:<12}  racks: {}  L1: {}  L2: {}  compute: {}  partitions: [{}]",
                    stats.racks.len(),
                    stats.n_l1,
                    stats.n_l2,
                    stats.n_compute,
                    partition_str,
                )?;
            }
        }

        // --- Containment tree ---
        let all_children: HashSet<&Id> = self.contains.values().flatten().collect();
        let mut roots: Vec<&Id> = self
            .contains
            .keys()
            .filter(|id| !all_children.contains(id))
            .collect();
        roots.sort_by(|a, b| a.0.cmp(&b.0));

        if !roots.is_empty() {
            writeln!(f, "  Containment tree:")?;
            for root in &roots {
                fmt_subtree(f, root, &self.contains, &self.entities, 2, 3)?;
            }
        }

        Ok(())
    }
}

/// Compress a list of switch/node names into range notation.
/// e.g. ["lrdn0001","lrdn0002","lrdn0003","lrdn0005"] → "lrdn[0001-0003,0005]"
/// Falls back to comma-separated if no common prefix is found.
fn compress_ids(ids: &[&Id]) -> String {
    if ids.is_empty() {
        return String::new();
    }

    // Split each id into (prefix, numeric_suffix)
    let parsed: Vec<(&str, Option<u64>)> = ids
        .iter()
        .map(|id| {
            let s = id.0.as_str();
            let split = s.len() - s.chars().rev().take_while(|c| c.is_ascii_digit()).count();
            let prefix = &s[..split];
            let num = s[split..].parse::<u64>().ok();
            (prefix, num)
        })
        .collect();

    // Check all share the same prefix and have numeric suffixes
    let prefix = parsed[0].0;
    let suffix_width = ids[0].0.len() - prefix.len();
    let all_same_prefix = parsed.iter().all(|(p, n)| *p == prefix && n.is_some());

    if !all_same_prefix {
        // Fallback: just list them
        return ids
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>()
            .join(", ");
    }

    let mut nums: Vec<u64> = parsed.iter().map(|(_, n)| n.unwrap()).collect();
    nums.sort_unstable();

    // Build ranges
    let mut ranges: Vec<String> = Vec::new();
    let mut range_start = nums[0];
    let mut range_end = nums[0];

    for &n in &nums[1..] {
        if n == range_end + 1 {
            range_end = n;
        } else {
            ranges.push(fmt_range(range_start, range_end, suffix_width));
            range_start = n;
            range_end = n;
        }
    }
    ranges.push(fmt_range(range_start, range_end, suffix_width));

    if ranges.len() == 1 && !ranges[0].contains('-') {
        // Single element, no brackets needed
        format!("{}{}", prefix, ranges[0])
    } else {
        format!("{}[{}]", prefix, ranges.join(","))
    }
}

fn fmt_range(start: u64, end: u64, width: usize) -> String {
    if start == end {
        format!("{:0>width$}", start)
    } else {
        format!("{:0>width$}-{:0>width$}", start, end)
    }
}

fn fmt_subtree(
    f: &mut fmt::Formatter<'_>,
    id: &Id,
    contains: &HashMap<Id, Vec<Id>>,
    entities: &HashMap<Id, Entity>,
    depth: usize,
    max_depth: usize,
) -> fmt::Result {
    let indent = "  ".repeat(depth);

    let kind_tag = match entities.get(id).map(|e| &e.kind) {
        Some(EntityKind::Switch { level: Some(0) }) => {
            let cell = entities
                .get(id)
                .and_then(|e| e.meta.get("cell"))
                .map(|s| s.as_str())
                .unwrap_or("?");
            let rack = entities
                .get(id)
                .and_then(|e| e.meta.get("rack"))
                .map(|s| s.as_str())
                .unwrap_or("?");
            format!("L1 cell={cell} rack={rack}")
        }
        Some(EntityKind::Switch { level: Some(1) }) => {
            let cell = entities
                .get(id)
                .and_then(|e| e.meta.get("cell"))
                .map(|s| s.as_str())
                .unwrap_or("?");
            let rack = entities
                .get(id)
                .and_then(|e| e.meta.get("rack"))
                .map(|s| s.as_str())
                .unwrap_or("?");
            format!("L2 cell={cell} rack={rack}")
        }
        Some(EntityKind::Switch { level: Some(l) }) => format!("switch(L{l})"),
        Some(EntityKind::Switch { level: None }) => "switch".to_string(),
        Some(EntityKind::Compute) => "compute".to_string(),
        Some(EntityKind::Group) => "group".to_string(),
        None => "?".to_string(),
    };

    writeln!(f, "{indent}[{kind_tag}] {}", id.0)?;

    if depth >= max_depth {
        if let Some(children) = contains.get(id) {
            if !children.is_empty() {
                writeln!(
                    f,
                    "{}  … ({} children, depth limit)",
                    indent,
                    children.len()
                )?;
            }
        }
        return Ok(());
    }

    let Some(children) = contains.get(id) else {
        return Ok(());
    };

    // Partition children by kind
    let mut compute_ids: Vec<&Id> = Vec::new();
    let mut l1_switch_ids: Vec<&Id> = Vec::new();
    let mut l2_switch_ids: Vec<&Id> = Vec::new();
    let mut other_ids: Vec<&Id> = Vec::new();

    for child_id in children {
        match entities.get(child_id).map(|e| &e.kind) {
            Some(EntityKind::Compute) => compute_ids.push(child_id),
            Some(EntityKind::Switch { level: Some(0) }) => l1_switch_ids.push(child_id),
            Some(EntityKind::Switch { level: Some(1) }) => l2_switch_ids.push(child_id),
            _ => other_ids.push(child_id),
        }
    }

    let child_indent = "  ".repeat(depth + 1);

    // Print each group as a single compressed range line
    if !l2_switch_ids.is_empty() {
        writeln!(
            f,
            "{child_indent}[L2 switches] {}",
            compress_ids(&l2_switch_ids)
        )?;
    }
    if !l1_switch_ids.is_empty() {
        writeln!(
            f,
            "{child_indent}[L1 switches] {}",
            compress_ids(&l1_switch_ids)
        )?;
    }
    if !compute_ids.is_empty() {
        let partitions = collect_partitions(&compute_ids, entities);
        let part_str = if partitions.is_empty() {
            String::new()
        } else {
            format!("  partitions=[{}]", partitions.join(", "))
        };
        writeln!(
            f,
            "{child_indent}[compute ×{}] {}{}",
            compute_ids.len(),
            compress_ids(&compute_ids),
            part_str,
        )?;
    }

    // Recurse only into non-leaf structural nodes (not compute, not leaf switches)
    for child_id in &other_ids {
        fmt_subtree(f, child_id, contains, entities, depth + 1, max_depth)?;
    }

    Ok(())
}

/// Collect the union of partitions from a set of compute nodes.
fn collect_partitions<'a>(ids: &[&'a Id], entities: &'a HashMap<Id, Entity>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for id in ids {
        if let Some(parts) = entities.get(id).and_then(|e| e.meta.get("partitions")) {
            for p in parts.split(',') {
                set.insert(p.to_string());
            }
        }
    }
    set.into_iter().collect()
}

/// Builder pattern for chaining multiple filters
pub struct FilterChain<'a> {
    ir: &'a TopologyIR,
    filters: Vec<Box<dyn Fn(&Entity) -> bool + 'a>>,
}

impl<'a> FilterChain<'a> {
    pub fn new(ir: &'a TopologyIR) -> Self {
        Self {
            ir,
            filters: Vec::new(),
        }
    }

    /// Add a filter condition
    pub fn filter<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&Entity) -> bool + 'a,
    {
        self.filters.push(Box::new(predicate));
        self
    }

    /// Keep only specified IDs
    pub fn keep_ids(self, ids: Vec<Id>) -> Self {
        let id_set: HashSet<Id> = ids.into_iter().collect();
        self.filter(move |e| id_set.contains(&e.id))
    }

    /// Remove specified IDs
    pub fn remove_ids(self, ids: Vec<Id>) -> Self {
        let id_set: HashSet<Id> = ids.into_iter().collect();
        self.filter(move |e| !id_set.contains(&e.id))
    }

    /// Build the filtered topology
    pub fn build(self) -> TopologyIR {
        self.ir
            .filter(|entity| self.filters.iter().all(|f| f(entity)))
    }
}
