# Dependency `aws_s3`: a per-service package that depends on `aws_core`. Its
# `import aws_core` is itself a cross-package dependency import, resolved against
# the same zenv — the transitive case.
import aws_core

def put(int64 n) -> int64:
    return aws_core.sign(n) + 100
