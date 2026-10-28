# A program importing a dependency package by its logical name (D-0052). The
# dependency resolves through `$ZAP_ZENV/pkg`, not the project's own src root.
import aws_s3

def main():
    print(aws_s3.put(5))
