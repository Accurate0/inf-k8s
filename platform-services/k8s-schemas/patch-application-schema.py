#!/usr/bin/env python3
"""Publish local helm chart values schemas and reference them from the Application schema.

Each charts/<name>/values.schema.json is copied to <output>/charts/<name>.json, then the
argoproj.io Application schema is rewritten so that every helm valuesObject node is an
anyOf over those chart schemas plus a permissive fallback. yaml-language-server resolves
the remote refs and aggregates completions across the branches, so editing a valuesObject
offers the chart's properties while Applications pointing at external charts still validate.
"""
import json
import os
import sys

BASE_URL = "https://k8s-schemas.anurag.sh"
APPLICATION_SCHEMA = "argoproj.io/application_v1alpha1.json"


def collect_chart_schemas(charts_dir: str) -> list[str]:
    names = []
    for entry in sorted(os.listdir(charts_dir)):
        if os.path.isfile(os.path.join(charts_dir, entry, "values.schema.json")):
            names.append(entry)
    return names


def publish_chart_schemas(charts_dir: str, names: list[str], dst: str) -> None:
    out_dir = os.path.join(dst, "charts")
    os.makedirs(out_dir, exist_ok=True)
    for name in names:
        with open(os.path.join(charts_dir, name, "values.schema.json")) as f:
            schema = json.load(f)
        schema["$id"] = f"{BASE_URL}/charts/{name}.json"
        with open(os.path.join(out_dir, f"{name}.json"), "w") as f:
            json.dump(schema, f, indent=2)


def values_object_node(names: list[str]) -> dict:
    branches = [{"$ref": f"{BASE_URL}/charts/{name}.json"} for name in names]
    branches.append({"type": "object"})
    return {
        "description": "Helm values. Local charts contribute completions; other charts are unconstrained.",
        "anyOf": branches,
    }


def patch(node, names: list[str]) -> int:
    patched = 0
    if isinstance(node, dict):
        for key, value in node.items():
            if key == "valuesObject" and isinstance(value, dict):
                node[key] = values_object_node(names)
                patched += 1
                continue
            patched += patch(value, names)
    elif isinstance(node, list):
        for item in node:
            patched += patch(item, names)
    return patched


def main(dst: str, charts_dir: str) -> None:
    names = collect_chart_schemas(charts_dir)
    if not names:
        sys.exit(f"ERROR: no values.schema.json found under {charts_dir}")

    publish_chart_schemas(charts_dir, names, dst)

    path = os.path.join(dst, APPLICATION_SCHEMA)
    if not os.path.isfile(path):
        sys.exit(f"ERROR: {APPLICATION_SCHEMA} not found at {path}")

    with open(path) as f:
        schema = json.load(f)

    patched = patch(schema, names)
    if patched == 0:
        sys.exit(f"ERROR: no valuesObject node found in {APPLICATION_SCHEMA}")

    with open(path, "w") as f:
        json.dump(schema, f, indent=2)

    print(f"Published {len(names)} chart schemas ({', '.join(names)})")
    print(f"Patched {patched} valuesObject nodes in {APPLICATION_SCHEMA}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
