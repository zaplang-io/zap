# Dependency `aws_core` (D-0011 §8): the shared package a service builds on.
# Materialized into zenv/pkg/aws_core by the package manager (D-0052); here it
# is pre-materialized so the fixture needs no network or resolve step.
def sign(int64 n) -> int64:
    return n + 1
