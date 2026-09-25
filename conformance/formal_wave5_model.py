from itertools import product

# A route update is admissible only if its epoch is newer than the current epoch.
for current, proposed in product(range(4), repeat=2):
    admitted = proposed > current
    if admitted:
        assert proposed >= current + 1
    else:
        assert proposed <= current

current = 2
for proposed in (1, 2):
    assert not (proposed > current)
assert 3 > current
print('formal_wave5_model: ok')
