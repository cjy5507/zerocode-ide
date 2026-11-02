import unittest

from topo import Cycle, impact, layers, order


class OrderTest(unittest.TestCase):
    def test_everything_comes_after_what_it_depends_on(self):
        self.assertEqual(order({"app": ["lib", "log"], "lib": ["core"], "log": ["core"], "core": []}),
                         ["core", "lib", "log", "app"])

    def test_a_node_named_only_as_a_dependency_counts(self):
        self.assertEqual(order({"a": ["b"]}), ["b", "a"])
        self.assertEqual(order({}), [])

    def test_the_smallest_ready_node_comes_next_whatever_the_dict_order(self):
        self.assertEqual(order({"z": [], "b": ["z"], "a": ["z"], "m": []}), ["m", "z", "a", "b"])
        self.assertEqual(order({"m": [], "a": ["z"], "b": ["z"], "z": []}), ["m", "z", "a", "b"])

    def test_a_newly_ready_small_node_goes_before_a_larger_one_waiting(self):
        self.assertEqual(order({"b": [], "c": [], "a": ["b"]}), ["b", "a", "c"])

    def test_repeated_dependencies_count_once(self):
        self.assertEqual(order({"a": ["b", "b"], "b": []}), ["b", "a"])


class LayerTest(unittest.TestCase):
    def test_groups(self):
        self.assertEqual(layers({"app": ["lib", "log"], "lib": ["core"], "log": ["core"], "core": []}),
                         [["core"], ["lib", "log"], ["app"]])

    def test_a_node_waits_for_its_slowest_dependency(self):
        self.assertEqual(layers({"d": ["a", "c"], "c": ["b"], "b": ["a"], "a": []}),
                         [["a"], ["b"], ["c"], ["d"]])

    def test_independent_nodes_share_the_first_layer(self):
        self.assertEqual(layers({"b": [], "a": [], "c": ["z"]}), [["a", "b", "z"], ["c"]])
        self.assertEqual(layers({}), [])


class CycleTest(unittest.TestCase):
    def path_of(self, graph, call=order):
        with self.assertRaises(Cycle) as caught:
            call(graph)
        return caught.exception.path

    def test_a_cycle_starts_from_its_smallest_node(self):
        self.assertEqual(self.path_of({"b": ["c"], "c": ["a"], "a": ["b"]}), ["a", "b", "c"])

    def test_a_node_depending_on_itself(self):
        self.assertEqual(self.path_of({"a": ["a"]}), ["a"])

    def test_the_shortest_cycle_is_named(self):
        graph = {"a": ["b"], "b": ["c"], "c": ["a"], "x": ["y"], "y": ["x"]}
        self.assertEqual(self.path_of(graph), ["x", "y"])

    def test_among_equally_short_cycles_the_first_in_list_order(self):
        graph = {"p": ["q"], "q": ["p"], "c": ["d"], "d": ["c"]}
        self.assertEqual(self.path_of(graph), ["c", "d"])

    def test_two_cycles_through_one_node(self):
        graph = {"a": ["c", "b"], "b": ["a"], "c": ["a"]}
        self.assertEqual(self.path_of(graph), ["a", "b"])

    def test_the_cycle_is_found_behind_clean_nodes(self):
        graph = {"top": ["m"], "m": ["n"], "n": ["m"], "free": []}
        self.assertEqual(self.path_of(graph), ["m", "n"])
        self.assertEqual(self.path_of(graph, layers), ["m", "n"])

    def test_the_path_follows_real_dependencies(self):
        graph = {"a": ["d"], "d": ["b"], "b": ["a"], "e": ["a"]}
        self.assertEqual(self.path_of(graph), ["a", "d", "b"])


class ImpactTest(unittest.TestCase):
    GRAPH = {"app": ["lib"], "lib": ["core"], "tool": ["core"], "core": [], "free": []}

    def test_direct_and_indirect(self):
        self.assertEqual(impact(self.GRAPH, "core"), ["app", "lib", "tool"])
        self.assertEqual(impact(self.GRAPH, "app"), [])

    def test_a_node_named_only_as_a_dependency(self):
        self.assertEqual(impact({"a": ["b"]}, "b"), ["a"])

    def test_an_unknown_node(self):
        with self.assertRaises(KeyError):
            impact(self.GRAPH, "nothing")

    def test_a_cycle_does_not_trap_it(self):
        self.assertEqual(impact({"a": ["b"], "b": ["a"], "c": ["a"]}, "a"), ["b", "c"])


if __name__ == "__main__":
    unittest.main()
