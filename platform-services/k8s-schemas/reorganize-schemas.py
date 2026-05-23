#!/usr/bin/env python3
"""Move openapi2jsonschema --expanded output into <group>/<kind>_<version>.json layout.

Each generated file is a self-contained resource schema (because --stand-alone was used).
We pick the destination from its x-kubernetes-group-version-kind extension so that the
filenames match the CRD layout (e.g. apps/deployment_v1.json, core/pod_v1.json).
"""
import json
import os
import shutil
import sys


def main(src: str, dst: str) -> None:
    for entry in sorted(os.listdir(src)):
        if not entry.endswith(".json"):
            continue
        path = os.path.join(src, entry)
        with open(path) as f:
            schema = json.load(f)
        for gvk in schema.get("x-kubernetes-group-version-kind") or []:
            group = gvk.get("group") or "core"
            kind = gvk["kind"].lower()
            version = gvk["version"]
            out_dir = os.path.join(dst, group)
            os.makedirs(out_dir, exist_ok=True)
            shutil.copy(path, os.path.join(out_dir, f"{kind}_{version}.json"))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
