#!/usr/bin/env python3
"""Build-time code inventory (TST-SUP-009, L-206).

    scripts/sup/build-inventory.py            # compare with supply-chain/build-inventory.json (exit 1 on a new entry)
    scripts/sup/build-inventory.py --update   # rewrite the baseline after review

The inventory lists every package in the resolved graph that has a build script (`custom-build` target) or is a proc macro. It covers two scopes:

- `all`: every package in Cargo.lock, for all targets;
- `linux`: the packages active for x86_64/aarch64-unknown-linux-gnu, as normal and build dependencies of the shipped binary. This scope comes from the feature-resolved `cargo tree`.

The gate fails if anything is added to the linux scope, or if an entry's version changes. A version change needs re-review: the baseline pins `name@version`.
"""
import json, subprocess, sys, os

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
BASE = os.path.join(ROOT, "supply-chain", "build-inventory.json")


MANIFEST = os.path.join(ROOT, "Cargo.toml")


def meta(platform=None):
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", MANIFEST]
    if platform:
        cmd += ["--filter-platform", platform]
    return json.loads(subprocess.run(cmd, check=True, capture_output=True).stdout)


def kinds(pkg):
    k = set()
    for t in pkg["targets"]:
        if "custom-build" in t["kind"]:
            k.add("build.rs")
        if "proc-macro" in t["kind"]:
            k.add("proc-macro")
    return k


def shipped_closure(m):
    """Packages reachable from searchdump via normal + build deps (no dev deps)."""
    nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
    root = next(p["id"] for p in m["packages"] if p["name"] == "searchdump-rs")
    seen, stack = set(), [root]
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(dk["kind"] in (None, "build") for dk in d["dep_kinds"]):
                stack.append(d["pkg"])
    return seen


def inventory():
    out = {"all": {}, "linux": {}}
    m = meta()
    for p in m["packages"]:
        k = kinds(p)
        if k and p["source"]:
            out["all"][f"{p['name']}@{p['version']}"] = sorted(k)
    for plat in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
        m = meta(plat)
        pk = {p["id"]: p for p in m["packages"]}
        for i in tree_closure(m, plat, MANIFEST):
            p = pk[i]
            k = kinds(p)
            if k and p["source"]:
                out["linux"][f"{p['name']}@{p['version']}"] = sorted(k)
    out["counts"] = {s: {"build.rs": sum("build.rs" in v for v in out[s].values()),
                         "proc-macro": sum("proc-macro" in v for v in out[s].values())} for s in ("all", "linux")}
    return out


def tree_closure(m, plat, manifest):
    """Real feature-resolved graph of the shipped binary (normal + build deps) via `cargo tree`."""
    out = subprocess.run(["cargo", "tree", "-p", "searchdump-rs", "-e", "normal,build", "--target", plat,
                          "--prefix", "none", "-f", "{p}", "--locked", "--manifest-path", manifest],
                         check=True, capture_output=True, text=True).stdout
    want = set()
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            want.add((parts[0], parts[1][1:]))
    return {p["id"] for p in m["packages"] if (p["name"], p["version"]) in want}


def main():
    global MANIFEST
    if "--dump" in sys.argv:  # print the inventory of another checkout (sbom-pr.sh)
        MANIFEST = sys.argv[sys.argv.index("--dump") + 1]
        json.dump(inventory(), sys.stdout, indent=1, sort_keys=True)
        return 0
    inv = inventory()
    if "--update" in sys.argv:
        old = json.load(open(BASE)) if os.path.exists(BASE) else {}
        inv["reviewed"] = old.get("reviewed", {})
        for k in inv["linux"]:
            inv["reviewed"].setdefault(k, "L-206 baseline 2026-10-29: reviewed at this version (see docs/SUPPLY-CHAIN.md)")
        inv["reviewed"] = {k: v for k, v in inv["reviewed"].items() if k in inv["linux"]}
        json.dump(inv, open(BASE, "w"), indent=1, sort_keys=True)
        print(f"baseline updated: {inv['counts']}")
        return 0
    base = json.load(open(BASE))
    new = sorted(set(inv["linux"]) - set(base["linux"]))
    gone = sorted(set(base["linux"]) - set(inv["linux"]))
    print(f"build-time code: all={inv['counts']['all']} linux={inv['counts']['linux']}")
    for g in gone:
        print(f"  removed (refresh baseline with --update): {g}")
    if new:
        for n in new:
            print(f"  NEW/CHANGED build-time code in the shipped graph, needs review: {n} {inv['linux'][n]}")
        return 1
    print("build inventory: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
