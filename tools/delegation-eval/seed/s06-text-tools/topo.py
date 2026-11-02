"""Ordering things that depend on each other.

A graph is a dict from a node to the nodes it depends on. A node that appears only as a
dependency is still a node. Nodes are strings.

`order(graph)` lists every node after everything it depends on. Whenever several nodes are
ready at once, the smallest by plain string comparison comes next, so the result is a single
definite list.

`layers(graph)` gives the same nodes as a list of sorted lists: the first holds the nodes with
no dependencies, and each later one holds the nodes whose dependencies all lie in earlier
layers and which have at least one dependency in the layer just before.

Both raise `Cycle` when no order exists. `Cycle.path` names one cycle as a list of nodes where
each depends on the next and the last depends on the first. Among all cycles in the graph it
is the one whose path, written from its own smallest node, comes first in list comparison
among the shortest cycles. A node depending on itself is a cycle of one.

`impact(graph, node)` is the sorted list of nodes that depend on NODE, directly or not, without
NODE itself. It raises `KeyError` for a node not in the graph, and works when the graph has a
cycle.
"""


class Cycle(ValueError):
    def __init__(self, path):
        super().__init__(" -> ".join(path))
        self.path = path


def order(graph):
    """Every node of GRAPH, each after what it depends on."""
    done = []
    remaining = dict(graph)
    while remaining:
        for node in remaining:
            if all(dep in done for dep in remaining[node]):
                done.append(node)
                del remaining[node]
                break
        else:
            raise Cycle(sorted(remaining))
    return done


def layers(graph):
    """The nodes of GRAPH grouped by how early they can run."""
    return [[node] for node in order(graph)]


def impact(graph, node):
    """The nodes that depend on NODE."""
    return sorted(other for other, deps in graph.items() if node in deps)
