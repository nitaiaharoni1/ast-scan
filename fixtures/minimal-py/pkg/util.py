"""Example module for dogfooding ast-scan."""


def add(a: int, b: int) -> int:
    if a < 0 or b < 0:
        return 0
    return a + b
