"""
Rust-based nodelist generator for Leonardo cluster.
Uses the topology_extractor Rust library for efficient query execution.
"""

import job_placer as jp
import random
from typing import Optional, List

class RustNodelistGenerator:
    def __init__(self, leonardo_file: str = "../leo.txt"):
        """Initialize with Leonardo topology file."""
        self.query_builder = jp.TopologyQueryBuilder(leonardo_file)
        self._compute_nodes = None
    
    def get_compute_nodes(self) -> List[str]:
        """Get all available compute nodes."""
        if self._compute_nodes is None:
            self._compute_nodes = self.query_builder.get_compute_nodes()
        return self._compute_nodes
    
    def get_random_anchor(self, exclude: Optional[List[str]] = None) -> str:
        """Get a random compute node to use as anchor."""
        nodes = self.get_compute_nodes()
        if exclude:
            nodes = [n for n in nodes if n not in exclude]
        
        if not nodes:
            raise ValueError("No available compute nodes")
        
        return random.choice(nodes)
    
    def get_nodelist_emulating_nanjing(
        self, 
        partition: str, 
        total_nodes: int,
        anchor: Optional[str] = None
    ) -> str:
        """
        Get nodelist emulating Nanjing topology.
        Pattern: alternating groups of 2 nodes at distances 2 and 4.
        
        Args:
            partition: SLURM partition (unused, for compatibility)
            total_nodes: Total number of nodes needed
            anchor: Optional anchor node, random if None
            
        Returns:
            Compressed SLURM nodelist string
        """
        if anchor is None:
            anchor = self.get_random_anchor()
        
        return self.query_builder.get_nodelist_emulating_nanjing(anchor, total_nodes)
    
    def get_nodelist_different_distances(
        self,
        partition: str,
        total_nodes: int,
        anchor: Optional[str] = None
    ) -> str:
        """
        Get nodelist with different distances pattern.
        Pattern: groups of 2 nodes at distances 2, 4, 5 (repeating).
        
        Args:
            partition: SLURM partition (unused, for compatibility)
            total_nodes: Total number of nodes needed
            anchor: Optional anchor node, random if None
            
        Returns:
            Compressed SLURM nodelist string
        """
        if anchor is None:
            anchor = self.get_random_anchor()
        
        return self.query_builder.get_nodelist_different_distances(anchor, total_nodes)
    
    def get_nodelist_custom_distances(
        self,
        partition: str,
        total_nodes: int,
        distances: List[tuple],
        anchor: Optional[str] = None,
        shared_parent: bool = False
    ) -> str:
        """
        Get nodelist with custom distance constraints.
        
        Args:
            partition: SLURM partition (unused, for compatibility)
            total_nodes: Total number of nodes needed
            distances: List of (count, distance) or (count, distance, parent_level) tuples
            anchor: Optional anchor node, random if None
            shared_parent: Whether to use shared parent constraint
            
        Returns:
            Compressed SLURM nodelist string
        """
        if anchor is None:
            anchor = self.get_random_anchor()
        
        if shared_parent:
            return self.query_builder.get_nodelist_distances_shared_parent(
                anchor, distances
            )
        else:
            return self.query_builder.get_nodelist_distances(anchor, distances)


# Compatibility functions for existing code
def get_nodelists_emulating_nanjing(
    generator: RustNodelistGenerator,
    partition: str,
    nodes: int,
    do_rank_nodelists: bool = False
) -> str:
    """Compatibility wrapper for existing code."""
    return generator.get_nodelist_emulating_nanjing(partition, nodes)


def get_nodelists_different_distances(
    generator: RustNodelistGenerator,
    partition: str,
    nodes: int,
    do_rank_nodelists: bool = False
) -> str:
    """Compatibility wrapper for existing code."""
    return generator.get_nodelist_different_distances(partition, nodes)


def get_nodelists_emulating_haicgu(
    generator: RustNodelistGenerator,
    partition: str,
    nodes: int,
    do_rank_nodelists: bool = False
) -> str:
    """
    Emulate HAICGU topology (customize based on your needs).
    For now, uses different distances pattern.
    """
    return generator.get_nodelist_different_distances(partition, nodes)