#!/usr/bin/env python3
"""Upstream-origin code inventory for the hostile-code review (TST-SUP-012, L-208).

    scripts/sup/upstream-lines.py [--checklist] [paths...]   (default: src)

A line is upstream-origin when `git blame -w` attributes it to a commit that is reachable from
`main` (the upstream mirror). With --checklist, it prints the upstream lines that match a review
checklist category:

- I/O targets;
- spawned work;
- swallowed errors;
- exit/panic paths;
- cancellation;
- logging.

REVIEW.md records the result. Any later diff that touches these lines needs re-review (CODEOWNERS on src/).
"""
import re, subprocess, sys
from collections import defaultdict

CHECKS = {
    "io": r"File::(create|open)|OpenOptions|fs::(write|remove|rename|create_dir|copy)|remove_file|rename\(|set_permissions|tokio::fs",
    "spawn": r"tokio::spawn|thread::spawn|spawn_blocking|Command::new|JoinSet|spawn\(",
    "swallow": r"let _ =|\.ok\(\)|unwrap_or_default|Err\(_\)\s*=>|\.unwrap_or\(|filter_map\(.*ok|catch_unwind",
    "exit": r"process::exit|panic!|unreachable!|\.unwrap\(\)|\.expect\(|abort\(",
    "cancel": r"\.abort\(\)|abort_handle|CancellationToken|select!|drop\(|timeout\(",
    "log": r"(println|eprintln|info|debug|warn|error|trace)!\(",
}


def upstream_commits():
    return set(subprocess.run(["git", "rev-list", "main"], capture_output=True, text=True, check=True).stdout.split())


def blame(path, ups):
    out = subprocess.run(["git", "blame", "-w", "--line-porcelain", path], capture_output=True, text=True).stdout
    lines, sha = [], None
    for l in out.splitlines():
        if re.match(r"^[0-9a-f]{40} ", l):
            sha = l.split()[0]
            ln = int(l.split()[2])
        elif l.startswith("\t"):
            lines.append((ln, sha in ups, l[1:]))
    return lines


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")] or ["src"]
    files = subprocess.run(["git", "ls-files", *args], capture_output=True, text=True).stdout.split()
    files = [f for f in files if f.endswith(".rs")]
    ups = upstream_commits()
    total = 0
    hits = defaultdict(list)
    per_file = {}
    for f in files:
        b = blame(f, ups)
        n = sum(1 for _, u, _ in b if u)
        if n:
            per_file[f] = (n, len(b))
        total += n
        if "--checklist" in sys.argv:
            for ln, u, text in b:
                if not u or text.strip().startswith("//"):
                    continue
                for k, rx in CHECKS.items():
                    if re.search(rx, text):
                        hits[k].append(f"{f}:{ln}: {text.strip()[:140]}")
    print(f"upstream-origin lines: {total}")
    for f, (n, t) in sorted(per_file.items(), key=lambda x: -x[1][0]):
        print(f"  {f}: {n}/{t}")
    for k in CHECKS:
        if hits[k]:
            print(f"\n[{k}] {len(hits[k])}")
            for h in hits[k]:
                print("  " + h)


if __name__ == "__main__":
    main()
