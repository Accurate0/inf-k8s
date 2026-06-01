#!/usr/bin/env python3
"""Re-evaluate k8s resource requests against actual Prometheus usage.

Scope: only hand-written manifests under platform-services/ and projects/.
Helm charts and system-components are not touched.

Queries Prometheus for per-container CPU and memory usage over a window,
joins against currently-configured requests, prints a recommendation
table, and (with --apply) edits the matching manifest files in place.

Policy:
  cpu_request    = max(p95_cpu * 1.25, 10m)        rounded to nearest 5m
  memory_request = max(p95_usage * 1.20, 16Mi)     rounded up to 8Mi

Both are sized off the p95 over the window (typical sustained usage), not the
worst spike. Memory defaults to container_memory_rss (anonymous memory, excludes
reclaimable page cache); pass --mem-metric workingset to size on rss + active
cache instead (the kubelet OOM/eviction basis).
"""

from __future__ import annotations
import argparse
import json
import math
import sys
import urllib.parse
import urllib.request
from collections import defaultdict
from pathlib import Path

try:
    from ruamel.yaml import YAML
    from ruamel.yaml.comments import CommentedMap
except ImportError:
    print("requires ruamel.yaml: pip install ruamel.yaml", file=sys.stderr)
    sys.exit(1)

_yaml = YAML()
_yaml.preserve_quotes = True
_yaml.indent(mapping=2, sequence=4, offset=2)
_yaml.width = 4096

PROM = "https://prometheus.inf-k8s.net"
WINDOW = "14d"  # default; override with --window
REPO = Path(__file__).resolve().parent.parent
SCOPE_DIRS = ["platform-services", "projects"]
WORKLOAD_KINDS = {"Deployment", "StatefulSet", "DaemonSet", "CronJob", "Job"}


def q(expr: str) -> list[dict]:
    url = f"{PROM}/api/v1/query?" + urllib.parse.urlencode({"query": expr})
    with urllib.request.urlopen(url, timeout=30) as r:
        data = json.load(r)
    if data["status"] != "success":
        raise RuntimeError(f"prom error: {data}")
    return data["data"]["result"]


def fmt_cpu(millicores: float) -> str:
    return f"{max(10, int(round(millicores / 5.0) * 5))}m"


def fmt_mem(bytes_: float) -> str:
    mib = max(16, int(math.ceil(bytes_ / (1024 * 1024) / 8.0) * 8))
    return f"{mib}Mi"


def collect_metrics(window: str = WINDOW) -> dict:
    rows: dict[tuple[str, str, str], dict] = defaultdict(dict)
    has_rule = bool(q("namespace_workload_pod:kube_pod_owner:relabel"))
    WINDOW_LOCAL = window

    def join(metric: str) -> str:
        if has_rule:
            return (f"sum by (namespace, workload, container) ("
                    f"{metric} * on(namespace, pod) group_left(workload) "
                    f"namespace_workload_pod:kube_pod_owner:relabel)")
        return f"sum by (namespace, pod, container) ({metric})"

    queries = {
        "cpu_req_mc": join('kube_pod_container_resource_requests{resource="cpu"}'),
        "mem_req_b": join('kube_pod_container_resource_requests{resource="memory"}'),
        "cpu_p95_cores": join(f'quantile_over_time(0.95, rate(container_cpu_usage_seconds_total{{container!="",container!="POD"}}[5m])[{WINDOW_LOCAL}:5m])'),
        "cpu_max_cores": join(f'max_over_time(rate(container_cpu_usage_seconds_total{{container!="",container!="POD"}}[5m])[{WINDOW_LOCAL}:5m])'),
        "mem_p95_b": join(f'quantile_over_time(0.95, container_memory_rss{{container!="",container!="POD"}}[{WINDOW_LOCAL}])'),
        "mem_ws_b": join(f'quantile_over_time(0.95, container_memory_working_set_bytes{{container!="",container!="POD"}}[{WINDOW_LOCAL}])'),
    }
    for name, expr in queries.items():
        for s in q(expr):
            m = s["metric"]
            k = (m.get("namespace", "?"),
                 m.get("workload") or m.get("pod", "?"),
                 m.get("container", "?"))
            v = float(s["value"][1])
            if name == "cpu_req_mc":
                v *= 1000
            rows[k][name] = v
    return rows


def load_docs(path: Path) -> list:
    with path.open() as f:
        return [d for d in _yaml.load_all(f) if isinstance(d, dict)]


def iter_docs(path: Path):
    try:
        for d in load_docs(path):
            yield d
    except Exception:
        return


def index_manifests() -> dict[tuple[str, str, str], tuple[Path, str]]:
    """Maps (namespace, workload_name, container_name) -> (file, container_name).

    Returns the same container name for clarity; the file is what we'll edit.
    """
    idx: dict[tuple[str, str, str], tuple[Path, str]] = {}
    for d in SCOPE_DIRS:
        for path in (REPO / d).rglob("*.yaml"):
            for doc in iter_docs(path):
                kind = doc.get("kind")
                if kind not in WORKLOAD_KINDS:
                    continue
                meta = doc.get("metadata") or {}
                name = meta.get("name")
                ns = meta.get("namespace")
                if not name or not ns:
                    continue
                if kind == "CronJob":
                    pod_spec = (((doc.get("spec") or {}).get("jobTemplate") or {})
                                .get("spec") or {}).get("template", {}).get("spec", {})
                else:
                    pod_spec = ((doc.get("spec") or {}).get("template") or {}).get("spec") or {}
                for ctr in (pod_spec.get("containers") or []) + (pod_spec.get("initContainers") or []):
                    cname = ctr.get("name")
                    if cname:
                        idx[(ns, name, cname)] = (path, cname)
    return idx


def _pod_spec(doc: dict) -> dict | None:
    spec = doc.get("spec") or {}
    if doc.get("kind") == "CronJob":
        return ((spec.get("jobTemplate") or {}).get("spec") or {}).get("template", {}).get("spec")
    return (spec.get("template") or {}).get("spec")


def edit_manifest(path: Path, container: str, new_cpu: str | None, new_mem: str | None) -> bool:
    """Round-trip the YAML with ruamel and set requests.cpu/memory for the
    named container. Preserves comments and most formatting.
    """
    docs = load_docs(path)
    changed = False
    for doc in docs:
        if doc.get("kind") not in WORKLOAD_KINDS:
            continue
        pod = _pod_spec(doc)
        if not pod:
            continue
        for ctrs_key in ("containers", "initContainers"):
            for ctr in pod.get(ctrs_key, []) or []:
                if ctr.get("name") != container:
                    continue
                res = ctr.get("resources")
                if res is None:
                    res = CommentedMap()
                    ctr["resources"] = res
                req = res.get("requests")
                if req is None:
                    req = CommentedMap()
                    res["requests"] = req
                if new_cpu and req.get("cpu") != new_cpu:
                    req["cpu"] = new_cpu
                    changed = True
                if new_mem and req.get("memory") != new_mem:
                    req["memory"] = new_mem
                    changed = True
    if changed:
        with path.open("w") as f:
            _yaml.dump_all(docs, f)
    return changed


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ns", help="filter to a namespace")
    ap.add_argument("--threshold", type=float, default=25.0,
                    help="flag cpu/mem as changed when it differs from current "
                         "by more than this percent (default 25)")
    ap.add_argument("--mem-metric", choices=("rss", "workingset"), default="rss",
                    help="basis for the memory request: rss = anonymous memory "
                         "(default, excludes reclaimable page cache); workingset = "
                         "rss + active cache (matches kubelet OOM/eviction accounting)")
    ap.add_argument("--window", default=WINDOW,
                    help=f"prometheus lookback (default {WINDOW}; e.g. 7d, 30d)")
    ap.add_argument("--apply", action="store_true",
                    help="edit manifests in place with the suggested values")
    ap.add_argument("--dry-run", action="store_true",
                    help="with --apply, only print what would change")
    args = ap.parse_args()
    the = args.threshold / 100.0

    metrics = collect_metrics(args.window)
    idx = index_manifests()

    suggestions = []
    for key, m in sorted(metrics.items()):
        ns, wl, ctr = key
        if args.ns and ns != args.ns:
            continue
        if key not in idx:
            continue  # outside platform-services/projects scope
        if "cpu_p95_cores" not in m and "mem_p95_b" not in m and "mem_ws_b" not in m:
            continue

        cur_cpu = m.get("cpu_req_mc")
        cur_mem = m.get("mem_req_b")
        mem_usage_b = m.get("mem_p95_b", 0) if args.mem_metric == "rss" else m.get("mem_ws_b", 0)
        sug_cpu_mc = max(m.get("cpu_p95_cores", 0) * 1000 * 1.25, 10)
        sug_mem_b = max(mem_usage_b * 1.20, 16 * 1024 * 1024)
        new_cpu = fmt_cpu(sug_cpu_mc)
        new_mem = fmt_mem(sug_mem_b)
        cpu_pct = (sug_cpu_mc - cur_cpu) / cur_cpu * 100 if cur_cpu else None
        mem_pct = (sug_mem_b - cur_mem) / cur_mem * 100 if cur_mem else None

        cpu_change = cur_cpu is None or (cur_cpu > 0 and abs(sug_cpu_mc - cur_cpu) / cur_cpu > the)
        mem_change = cur_mem is None or (cur_mem > 0 and abs(sug_mem_b - cur_mem) / cur_mem > the)
        change = "+".join(p for p, c in (("cpu", cpu_change), ("mem", mem_change)) if c) or "-"

        path, cname = idx[key]
        suggestions.append({
            "key": key, "path": path, "container": cname, "change": change,
            "cur_cpu": f"{int(cur_cpu)}m" if cur_cpu else "-",
            "cpu_usage": f"{int(m.get('cpu_p95_cores', 0)*1000)}m",
            "new_cpu": new_cpu, "cpu_change": cpu_change,
            "cpu_pct": f" ({cpu_pct:+.0f}%)" if cpu_pct is not None else "",
            "cur_mem": f"{int(cur_mem/1024/1024)}Mi" if cur_mem else "-",
            "mem_usage": f"{int(mem_usage_b/1024/1024)}Mi",
            "new_mem": new_mem, "mem_change": mem_change,
            "mem_pct": f" ({mem_pct:+.0f}%)" if mem_pct is not None else "",
        })

    # Print table
    cols = [("workload", lambda r: r["key"][1]),
            ("container", lambda r: r["container"]),
            ("change", lambda r: r["change"]),
            ("cur_cpu", lambda r: r["cur_cpu"]),
            ("cpu_usage", lambda r: r["cpu_usage"]),
            ("new_cpu", lambda r: r["new_cpu"] + r["cpu_pct"]),
            ("cur_mem", lambda r: r["cur_mem"]),
            ("mem_usage", lambda r: r["mem_usage"]),
            ("new_mem", lambda r: r["new_mem"] + r["mem_pct"])]
    rendered = [[fn(r) for _, fn in cols] for r in suggestions]
    widths = [max(len(h), *(len(row[i]) for row in rendered)) if rendered else len(h)
              for i, (h, _) in enumerate(cols)]
    headers = [h for h, _ in cols]
    print("  ".join(h.ljust(w) for h, w in zip(headers, widths)))
    print("  ".join("-" * w for w in widths))
    for row in rendered:
        print("  ".join(c.ljust(w) for c, w in zip(row, widths)))

    needs = sum(1 for r in suggestions if r["change"] != "-")
    print(f"\n{len(suggestions)} workloads in scope, {needs} need adjustment "
          f"(window={args.window}, threshold={args.threshold:g}%, mem={args.mem_metric}).",
          file=sys.stderr)

    if args.apply:
        print("\n--- applying ---", file=sys.stderr)
        for r in suggestions:
            new_cpu = r["new_cpu"] if r["cpu_change"] else None
            new_mem = r["new_mem"] if r["mem_change"] else None
            if not (new_cpu or new_mem):
                continue
            rel = r["path"].relative_to(REPO)
            if args.dry_run:
                print(f"would edit {rel}  [{r['container']}] cpu={new_cpu} mem={new_mem}",
                      file=sys.stderr)
                continue
            ok = edit_manifest(r["path"], r["container"], new_cpu, new_mem)
            tag = "edited" if ok else "no-op"
            print(f"{tag:7s} {rel}  [{r['container']}] cpu={new_cpu} mem={new_mem}",
                  file=sys.stderr)


if __name__ == "__main__":
    main()
