import random
from typing import Optional, List, Tuple, Union
import job_placer as jp  # Your PyO3 bindings

class TopologyNodelistGenerator:
    def __init__(self, query_builder: jp.TopologyQueryBuilder):
        """
        Initializes the generator with a pre-configured Rust QueryBuilder.
        """
        self.query_builder = query_builder
        self._compute_nodes: Optional[List[str]] = None

    def filter_by_ids(self, ids: List[str]):
        """
        Hard filter: Keep only the nodes with these specific IDs.
        """
        self.query_builder.filter_by_ids(ids)
        self._compute_nodes = None  # Clear cache so it re-fetches from Rust

    def get_compute_nodes(self) -> List[str]:
        """Return all compute nodes, cached after the first call."""
        if self._compute_nodes is None:
            self._compute_nodes = self.query_builder.get_compute_nodes()
        return self._compute_nodes

    def get_random_anchor(self, exclude: Optional[List[str]] = None) -> str:
        """Pick a random compute node as an anchor point."""
        nodes = self.get_compute_nodes()
        if exclude:
            nodes = [n for n in nodes if n not in exclude]
        if not nodes:
            raise ValueError("No available compute nodes found in the topology.")
        return random.choice(nodes)

    def get_nodelist(
        self,
        filter_ids: Optional[List[str]] = None,
        distances: Optional[Union[List[Tuple[int, float]], List[Tuple[int, float, int]]]] = None,
        anchor: Optional[str] = None,
        shared_parent: bool = False
    ) -> List[str]:
        """
        Request a list of nodes based on distance constraints.
        
        Args:
            filter_ids: List of node IDs to filter by before generating the nodelist.
            distances: 
                If shared_parent=False: List of (count, distance)
                If shared_parent=True:  List of (count, distance, parent_level)
            anchor: The starting node ID. Picks random if None.
            shared_parent: Toggle which Rust query engine to use.
        """
        if filter_ids:
            self.filter_by_ids(filter_ids)

        if anchor is None:
            anchor = self.get_random_anchor()

        # Check if the node exists before calling Rust to get better error messages
        if not self.query_builder.is_valid_compute_node(anchor):
            raise ValueError(f"Anchor node '{anchor}' is not a valid compute node.")

        # Call the specific Rust binding based on the shared_parent flag
        try:
            if shared_parent:
                # Expects List[Tuple[int, float, int]]
                return self.query_builder.get_nodelist_distances_shared_parent(anchor, distances)
            else:
                # Expects List[Tuple[int, float]]
                return self.query_builder.get_nodelist_distances(anchor, distances)
        except Exception as e:
            # Catching PyRuntimeError or TypeError if tuple sizes don't match
            raise RuntimeError(f"Topology query failed: {e}")

class LeonardoNodelistGenerator(TopologyNodelistGenerator):
    """Specialized generator for the Leonardo Supercomputer."""
    def __init__(self, leonardo_file: Optional[str] = None):
        # Matches Rust: new(parser: Option<String>, path: Option<String>)
        # If leonardo_file is None, Rust receives (Some("leonardo"), None)
        qb = jp.TopologyQueryBuilder("leonardo", leonardo_file)
        super().__init__(qb)

class ManualNodelistGenerator(TopologyNodelistGenerator):
    """Specialized generator for a manual IR file."""
    def __init__(self, ir_file: str):
        # Matches Rust: ("manual", Some(p))
        qb = jp.TopologyQueryBuilder("manual", ir_file)
        super().__init__(qb)