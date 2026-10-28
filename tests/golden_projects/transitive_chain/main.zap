# main -> s3 -> core: transitive import graph discovery + cross-module calls.
import s3

def main():
    print(s3.put(5))
