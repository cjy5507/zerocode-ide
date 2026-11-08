"""Geometry helpers for the layout engine."""

from legacy_math import legacy_hypot, legacy_sqrt


def diagonal(width, height):
    """Length of a rectangle's diagonal."""
    return legacy_hypot(width, height)


def side_of_square(area):
    """Side of a square with this area."""
    return legacy_sqrt(area)
