#!/usr/bin/env python3
"""Publish local helm chart values schemas and reference them from the HelmRelease schema.

Each charts/<name>/values.schema.json is copied to <output>/charts/<name>.json, then the
Flux HelmRelease schema is rewritten so that spec.values is an anyOf over those chart
schemas plus a permissive fallback. yaml-language-server resolves the remote refs and
aggregates completions across the branches, so editing spec.values offers the chart's
properties while HelmReleases pointing at external charts still validate.
"""
import json
import os
import sys

BASE_URL = "https://k8s-schemas.anurag.sh"
HELMRELEASE_SCHEMA = "helm.toolkit.fluxcd.io/helmrelease_v2.json"


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
        "description": "Values holds the values for this Helm release. Local charts contribute completions; other charts are unconstrained.",
        "anyOf": branches,
    }


def patch(schema: dict, names: list[str]) -> bool:
    spec = schema.get("properties", {}).get("spec", {}).get("properties", {})
    if not isinstance(spec.get("values"), dict):
        return False
    spec["values"] = values_object_node(names)
    return True


def main(dst: str, charts_dir: str) -> None:
    names = collect_chart_schemas(charts_dir)
    if not names:
        sys.exit(f"ERROR: no values.schema.json found under {charts_dir}")

    publish_chart_schemas(charts_dir, names, dst)

    path = os.path.join(dst, HELMRELEASE_SCHEMA)
    if not os.path.isfile(path):
        sys.exit(f"ERROR: {HELMRELEASE_SCHEMA} not found at {path}")

    with open(path) as f:
        schema = json.load(f)

    if not patch(schema, names):
        sys.exit(f"ERROR: no spec.values node found in {HELMRELEASE_SCHEMA}")

    with open(path, "w") as f:
        json.dump(schema, f, indent=2)

    print(f"Published {len(names)} chart schemas ({', '.join(names)})")
    print(f"Patched spec.values in {HELMRELEASE_SCHEMA}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
