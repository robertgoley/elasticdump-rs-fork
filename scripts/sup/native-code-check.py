#!/usr/bin/env python3
"""Native-code allow-list (TST-SUP-021, L-213).

Fails when a crate in the shipped Linux graph (normal + build deps of elasticdump-rs, x86_64 and
aarch64 gnu) brings native code and is not on the allow-list. Native code is any of:
  * a `links = ...` key,
  * a build dependency on cc / cmake / bindgen / pkg-config / autotools,
  * C/C++/assembly sources in the package (.c .cc .cpp .h .S .s .asm).
    Perl/asm generators are counted through ring's pregenerated files.
Allow-list (TEST-REQUIREMENTS SUP-021): zstd-sys, ring.
The graph is the feature-resolved one from `cargo tree -e normal,build --target <linux>`.
"""
import json, os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ALLOW = {"zstd-sys": "libzstd (vendored C), reviewed L-213", "ring": "BoringSSL-derived C/asm crypto, reviewed L-213",
         # L-808b (user decision): mimalloc is the default global allocator on *-linux-gnu.
         "libmimalloc-sys": "mimalloc (vendored C, MIT), default allocator on glibc Linux, added L-808b"}
NATIVE_BUILD = {"cc", "cmake", "bindgen", "pkg-config", "autotools", "vcpkg"}
EXT = (".c", ".cc", ".cpp", ".cxx", ".h", ".hpp", ".S", ".s", ".asm")


def meta(plat):
    return json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", plat,
                                      "--manifest-path", os.path.join(ROOT, "Cargo.toml")],
                                     check=True, capture_output=True).stdout)


def closure(m):
    nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
    root = next(p["id"] for p in m["packages"] if p["name"] == "elasticdump-rs")
    seen, st = set(), [root]
    while st:
        i = st.pop()
        if i in seen:
            continue
        seen.add(i)
        st += [d["pkg"] for d in nodes[i]["deps"] if any(k["kind"] in (None, "build") for k in d["dep_kinds"])]
    return seen


def sources(pkgdir):
    n = 0
    for dp, dn, fn in os.walk(pkgdir):
        dn[:] = [d for d in dn if d not in ("tests", "benches", "examples", "target")]
        n += sum(f.endswith(EXT) for f in fn)
    return n


def tree_closure(m, plat, manifest):
    """Real feature-resolved graph of the shipped binary (normal + build deps) via `cargo tree`."""
    out = subprocess.run(["cargo", "tree", "-p", "elasticdump-rs", "-e", "normal,build", "--target", plat,
                          "--prefix", "none", "-f", "{p}", "--locked", "--manifest-path", manifest],
                         check=True, capture_output=True, text=True).stdout
    want = set()
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            want.add((parts[0], parts[1][1:]))
    return {p["id"] for p in m["packages"] if (p["name"], p["version"]) in want}


def main():
    found = {}
    for plat in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
        m = meta(plat)
        pk = {p["id"]: p for p in m["packages"]}
        for i in tree_closure(m, plat, os.path.join(ROOT, "Cargo.toml")):
            p = pk[i]
            if not p["source"] or p["name"] in NATIVE_BUILD:
                continue  # path crates; the build-tool crates themselves (their users are flagged)
            why = []
            if p.get("links"):
                why.append(f"links={p['links']}")
            bdeps = {d["name"] for d in p["dependencies"] if d["kind"] == "build"} & NATIVE_BUILD
            if bdeps:
                why.append("build-deps=" + ",".join(sorted(bdeps)))
            n = sources(os.path.dirname(p["manifest_path"]))
            if n:
                why.append(f"{n} C/asm files")
            if why:
                found[f"{p['name']}@{p['version']}"] = (p["name"], why)
    bad = 0
    for k, (name, why) in sorted(found.items()):
        if name in ALLOW:
            print(f"allowed  {k}: {'; '.join(why)}  [{ALLOW[name]}]")
        else:
            print(f"NOT ALLOWED {k}: {'; '.join(why)}")
            bad = 1
    print("native-code check:", "FAIL" if bad else "OK")
    return bad


if __name__ == "__main__":
    sys.exit(main())
