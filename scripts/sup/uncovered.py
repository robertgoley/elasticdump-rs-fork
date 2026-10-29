#!/usr/bin/env python3
"""List functions in src/ that never ran (TST-SUP-008, L-205).

    scripts/sup/uncovered.py COV.json [--root DIR] > report.md

COV.json is the output of `cargo llvm-cov report --json` after T0 (unit), T1
(contract) and T2 (it) have run with `--no-report`; scripts/sup/coverage.sh does all of it.
Generic instantiations are merged: a function counts as covered if any
instantiation ran.
"""
import json, re, shutil, subprocess, sys
from collections import defaultdict

ESC = {"$LT$": "<", "$GT$": ">", "$RF$": "&", "$BP$": "*", "$C$": ",", "$SP$": "@", "$u20$": " ",
       "$u27$": "'", "$u5b$": "[", "$u5d$": "]", "$u7b$": "{", "$u7d$": "}", "$u3b$": ";", "$u2b$": "+",
       "$u22$": '"', "$u7e$": "~"}


def demangle(sym):
    m = re.match(r"^_ZN(.*)E$", sym)
    if not m:
        return sym
    s, parts, i = m.group(1), [], 0
    while i < len(s) and s[i].isdigit():
        j = i
        while s[j].isdigit():
            j += 1
        n = int(s[i:j])
        parts.append(s[j:j + n])
        i = j + n
    if parts and re.fullmatch(r"h[0-9a-f]{16}", parts[-1]):
        parts.pop()
    out = "::".join(parts)
    for k, v in ESC.items():
        out = out.replace(k, v)
    return out.replace("..", "::").lstrip("_")


def demangle_all(syms):
    if shutil.which("rustfilt"):
        out = subprocess.run(["rustfilt"], input="\n".join(syms), capture_output=True, text=True).stdout.split("\n")
        return [re.sub(r"::h[0-9a-f]{16}$", "", o) for o in out[:len(syms)]]
    return [demangle(x) for x in syms]


def main():
    path = sys.argv[1]
    root = sys.argv[sys.argv.index("--root") + 1] if "--root" in sys.argv else ""
    data = json.load(open(path))["data"][0]
    seen = defaultdict(lambda: [0, None, None])  # (file, line, name) -> [max count]
    for f in data["functions"]:
        files = [x for x in f["filenames"] if "/src/" in x and "/.cargo/" not in x and "/rustc/" not in x]
        if not files:
            continue
        fn = files[0].split(root, 1)[-1].lstrip("/") if root else files[0]
        line = f["regions"][0][0] if f["regions"] else 0
        name = f["name"].split(":")[-1]
        key = (fn, line)
        e = seen[key]
        e[0] = max(e[0], f["count"])
        e[1] = name if e[1] is None or len(name) < len(e[1]) else e[1]
    unc = sorted((k, v[1]) for k, v in seen.items() if v[0] == 0)
    names = demangle_all([n for _, n in unc])
    unc = [(k, n) for (k, _), n in zip(unc, names)]
    named = [(k, n) for k, n in unc if "{closure" not in n]
    closures = defaultdict(int)
    for (fn, _), n in unc:
        if "{closure" in n:
            closures[fn] += 1
    tot = data["totals"]
    print("# Never-executed functions (T0 + T1 + T2)\n")
    print(f"Function coverage {tot['functions']['percent']:.2f}% ({tot['functions']['covered']}/{tot['functions']['count']}), "
          f"lines {tot['lines']['percent']:.2f}%, regions {tot['regions']['percent']:.2f}%. "
          f"{len(unc)} source functions never ran (generic instantiations merged).\n")
    print(f"## Named functions ({len(named)})\n")
    print("| File:line | Function | Review |\n|---|---|---|")
    for (fn, line), name in named:
        print(f"| `{fn}:{line}` | `{name}` | |")
    print(f"\n## Closures never run ({sum(closures.values())})\n")
    print("These are almost all `.with_context(|| …)`/`map_err` error-message closures and the clap derive's error closures. They run only on the matching error path.\n")
    print("| File | Closures |\n|---|---|")
    for fn in sorted(closures):
        print(f"| `{fn}` | {closures[fn]} |")


if __name__ == "__main__":
    main()
