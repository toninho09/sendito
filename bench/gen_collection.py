#!/usr/bin/env python3
"""Generates the benchmark collection: 1,000 requests in 50 folders,
5 environments x 100 variables, 10,000 history entries, plus a request
that fetches a ~10 MB JSON document from the echo server."""
import json, os, sys

root = sys.argv[1] if len(sys.argv) > 1 else "bench/collection"
port = sys.argv[2] if len(sys.argv) > 2 else "8787"
os.makedirs(f"{root}/.sendito/environments", exist_ok=True)
open(f"{root}/.sendito/config.toml", "w").write('[collection]\nname = "bench"\n')
open(f"{root}/.sendito/collection.toml", "w").write("[variables]\n" + "".join(f'coll_{i} = "c{i}"\n' for i in range(20)))
methods = ["GET", "POST", "PUT", "PATCH", "DELETE"]
n = 0
for f in range(50):
    d = f"{root}/service-{f:02d}"
    os.makedirs(d, exist_ok=True)
    for r in range(20):
        m = methods[(f + r) % 5]
        body = ""
        if m in ("POST", "PUT", "PATCH"):
            body = f'\n[body]\ntype = "json"\ntext = \'\'\'\n{{\n  "id": {r},\n  "name": "item {r}",\n  "tags": ["a", "b"]\n}}\n\'\'\'\n'
        text = (f'kind = "http"\nmethod = "{m}"\nurl = "{{{{base_url}}}}/svc{f}/items/{r}"\n\n'
                f'[[params]]\nname = "page"\nvalue = "{r}"\n\n[[headers]]\nname = "X-Trace"\nvalue = "{{{{var_{r}}}}}"\n'
                f'\n[auth]\ntype = "bearer"\ntoken = "{{{{token}}}}"\n' + body)
        open(f"{d}/request {r:02d}.toml", "w").write(text)
        n += 1
for e in range(5):
    lines = ["[variables]", f'base_url = "http://127.0.0.1:{port}"', 'token = "t"']
    lines += [f'var_{i} = "env{e}-value-{i}"' for i in range(98)]
    open(f"{root}/.sendito/environments/env{e}.toml", "w").write("\n".join(lines) + "\n")
open(f"{root}/big json.toml", "w").write(f'kind = "http"\nmethod = "GET"\nurl = "http://127.0.0.1:{port}/json/10000000"\n')
with open(f"{root}/.sendito/history.jsonl", "w") as h:
    for i in range(10000):
        h.write(json.dumps({"ts": "2026-11-21T10:00:00Z", "name": f"request {i % 20:02d}", "path": f"service-{i % 50:02d}/request {i % 20:02d}.toml",
                            "env": "env0", "badge": methods[i % 5], "target": f"http://127.0.0.1:{port}/svc/items/{i}",
                            "request": "kind = \"http\"\nmethod = \"GET\"\nurl = \"{{base_url}}/x\"\n", "status": 200, "duration_ms": 3, "size": 120}) + "\n")
open(f"{root}/.sendito/session.toml", "w").write('active_env = "env0"\nactive_tab = 0\n\n[[tabs]]\npath = "big json.toml"\nname = "big json"\n')
print(f"{n + 1} requests in 50 folders + root at {root}")
