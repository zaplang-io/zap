# Golden (#1064): the same strings as hash_wyhash_seed1.zap but under
# ZAPHASHSEED=2 (sibling .env). Every value differs from the seed-1 fixture,
# demonstrating that the per-process hash seed changes str hashes across
# independent processes (the Python-like randomization this ticket adds) while
# staying stable within a single run. A persisted str hash is therefore invalid
# across processes; this is exactly why goldens pin the seed.
def main():
    print("abc".__hash__())  # -6233211153433283820
    print("".__hash__())  # 2615077062016060165
    print("hello world".__hash__())  # -909809627091895813
