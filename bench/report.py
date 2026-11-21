#!/usr/bin/env python3
"""Summarizes bench/run.sh results against the TARGET thresholds."""
import json, re, sys, os
out = sys.argv[1] if len(sys.argv) > 1 else "bench/results"

def stats(v):
    v = sorted(v)
    n = len(v)
    return sum(v) / n, v[min(n - 1, int(0.99 * n))], v[-1], n

def rss_kb(path):
    t = open(path).read()
    m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", t)
    if m:
        return int(m.group(1))
    m = re.search(r"(\d+)\s+maximum resident set size", t)  # macOS, bytes
    return int(m.group(1)) // 1024 if m else None

lines = []
ff = [int(l.split()[3]) / 1000 for l in open(f"{out}/first-frame-us.txt") if l.startswith("first frame")]
m, p99, mx, n = stats(ff)
lines.append(f"| Cold start → first frame (in-process, {n} runs) | {m:.1f} ms | {p99:.1f} ms | {mx:.1f} ms | ≤ 100 ms | {'PASS' if mx <= 100 else 'FAIL'} |")
try:
    h = json.load(open(f"{out}/startup-hyperfine.json"))
    def walls(r):
        if "measurements" in r:
            return [m["time_wall_clock"]["value"] for m in r["measurements"]]
        return r["times"]
    times = walls(h["results"][1])
    btimes = walls(h["results"][0])
    m2, p2, x2, n2 = stats([t * 1000 for t in times])
    mb = sum(btimes) / len(btimes) * 1000
    lines.append(f"| Cold start, end to end through a pty wrapper (hyperfine, {n2} runs; the wrapper alone takes {mb:.1f} ms) | {m2:.1f} ms | {p2:.1f} ms | {x2:.1f} ms | ≤ 100 ms | {'PASS' if x2 <= 100 else 'FAIL'} |")
except Exception as e:
    lines.append(f"| hyperfine | error: {e} | | | | |")
idle = rss_kb(f"{out}/rss-idle.txt")
big = rss_kb(f"{out}/rss-10mb.txt")
lines.append(f"| Max RSS, idle with the 1,000-request collection | {idle/1024:.1f} MB | | | ≤ 30 MB | {'PASS' if idle/1024 <= 30 else 'FAIL'} |")
lines.append(f"| Max RSS, showing a 10 MB JSON response | {big/1024:.1f} MB | | | ≤ 120 MB | {'PASS' if big/1024 <= 120 else 'FAIL'} |")
frames, events, msgs = [], [], []
for l in open(f"{out}/frames.log"):
    k, v = l.split()
    {"frame": frames, "event": events, "msg": msgs}[k].append(int(v) / 1000)
m, p99, mx, n = stats(frames)
lines.append(f"| Frame render during 60 s scroll/type ({n} frames) | {m:.2f} ms | {p99:.2f} ms | {mx:.2f} ms | p99 ≤ 16 ms | {'PASS' if p99 <= 16 else 'FAIL'} |")
handlers = events + msgs
m, p99, mx, n = stats(handlers)
lines.append(f"| Longest event / async-message handling on the UI thread ({n}) | {m:.2f} ms | {p99:.2f} ms | {mx:.2f} ms | ≤ 50 ms | {'PASS' if mx <= 50 else 'FAIL'} |")
report = "| Measurement | Mean | p99 | Max | Target | Result |\n|---|---|---|---|---|---|\n" + "\n".join(lines) + "\n"
print(report)
open(f"{out}/REPORT.md", "w").write(report)
