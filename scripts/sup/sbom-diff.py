#!/usr/bin/env python3
"""Diff two CycloneDX SBOMs (TST-SUP-011, L-207).

    sbom-diff.py BASE.cdx.json HEAD.cdx.json [--inv-base A.json --inv-head B.json]
                 [--image-base X.cdx.json --image-head Y.cdx.json] [--json OUT]

The report is Markdown on stdout. It lists:

- crates added, removed and upgraded/downgraded;
- licence changes;
- new build scripts and proc macros (from build-inventory JSON);
- image package changes (from syft CycloneDX).

Exit code: 0 when the diff is empty, 10 when it is not. CI then requires the `sbom-reviewed` label.
"""
import argparse, json, sys


def comps(path):
    d = json.load(open(path))
    out = {}
    def walk(cs):
        for c in cs or []:
            if c.get("type") in ("library", "application", "operating-system", None) and c.get("name"):
                lic = sorted({(l.get("license") or {}).get("id") or (l.get("license") or {}).get("name") or l.get("expression") or ""
                              for l in c.get("licenses") or []} - {""})
                out.setdefault(c["name"], {})[c.get("version", "")] = ",".join(lic)
            walk(c.get("components"))
    walk(d.get("components"))
    return out


def diff(base, head):
    rows = {"added": [], "removed": [], "changed": [], "licence": []}
    for n in sorted(set(base) | set(head)):
        b, h = base.get(n, {}), head.get(n, {})
        if not b:
            rows["added"] += [f"{n}@{v} ({l or '?'})" for v, l in sorted(h.items())]
        elif not h:
            rows["removed"] += [f"{n}@{v}" for v in sorted(b)]
        elif set(b) != set(h):
            rows["changed"].append(f"{n}: {', '.join(sorted(b))} -> {', '.join(sorted(h))}")
        for v in set(b) & set(h):
            if b[v] != h[v]:
                rows["licence"].append(f"{n}@{v}: {b[v] or '?'} -> {h[v] or '?'}")
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("base"); ap.add_argument("head")
    ap.add_argument("--inv-base"); ap.add_argument("--inv-head")
    ap.add_argument("--image-base"); ap.add_argument("--image-head")
    ap.add_argument("--json")
    a = ap.parse_args()
    rep = {"crates": diff(comps(a.base), comps(a.head))}
    if a.inv_base and a.inv_head:
        ib, ih = json.load(open(a.inv_base))["linux"], json.load(open(a.inv_head))["linux"]
        rep["build_time"] = {"added": sorted(f"{k} {ih[k]}" for k in set(ih) - set(ib)),
                             "removed": sorted(set(ib) - set(ih))}
    if a.image_base and a.image_head:
        rep["image"] = diff(comps(a.image_base), comps(a.image_head))
    nonempty = any(v for sec in rep.values() for v in sec.values())
    rep["nonempty"] = nonempty
    print("## SBOM diff\n")
    if not nonempty:
        print("No dependency, licence, build-time or image package changes.")
    for sec, d in rep.items():
        if sec == "nonempty":
            continue
        for k, items in d.items():
            if items:
                print(f"**{sec} / {k}** ({len(items)})\n")
                for i in items:
                    print(f"- `{i}`")
                print()
    if a.json:
        json.dump(rep, open(a.json, "w"), indent=1)
    return 10 if nonempty else 0


if __name__ == "__main__":
    sys.exit(main())
