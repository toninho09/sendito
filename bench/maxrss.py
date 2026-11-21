#!/usr/bin/env python3
"""Runs a command and writes its maximum resident set size like GNU time -v
("Maximum resident set size (kbytes): N"). Used when /usr/bin/time is missing;
it reads the same wait4() rusage that time(1) reports."""
import os, subprocess, sys, resource
out = sys.argv[2] if sys.argv[1] == "-o" else None
cmd = sys.argv[3:] if out else sys.argv[1:]
p = subprocess.Popen(cmd)
_, status, ru = os.wait4(p.pid, 0)
text = f"Command: {' '.join(cmd)}\nMaximum resident set size (kbytes): {ru.ru_maxrss}\nExit status: {os.waitstatus_to_exitcode(status)}\n"
if out:
    open(out, "w").write(text)
else:
    sys.stderr.write(text)
