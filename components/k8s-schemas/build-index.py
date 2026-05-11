#!/usr/bin/env python3
"""Build index.json: a sorted list of all schema paths under the output dir."""
import json
import os
import sys


def main(root: str) -> None:
    paths = []
    for dirpath, _, files in os.walk(root):
        rel_dir = os.path.relpath(dirpath, root)
        if rel_dir == "master-standalone" or rel_dir.startswith("master-standalone" + os.sep):
            continue
        for f in files:
            if f.endswith(".json") and f != "index.json":
                rel = f if rel_dir == "." else os.path.join(rel_dir, f)
                paths.append(rel.replace(os.sep, "/"))
    paths.sort()
    with open(os.path.join(root, "index.json"), "w") as fh:
        json.dump(paths, fh, indent=2)
    print(f"Indexed {len(paths)} schemas")


if __name__ == "__main__":
    main(sys.argv[1])
