#!/usr/bin/env python3
"""SUP-019: crate publisher/ownership snapshot and diff.

  publishers.py snapshot <cargo-supply-chain.json> [-o supply-chain/publishers.json]
      normalise `cargo supply-chain json` output to {crate: [owner logins]}
  publishers.py diff <old snapshot> <new snapshot|cargo-supply-chain.json> [--markdown]
      report owner changes. Exit 10 when an existing crate gained or lost an
      owner (the alert), 0 otherwise. New/removed crates are listed but do not
      alert (they are reviewed through the SBOM diff, SUP-020).

Typical weekly run:
  cargo supply-chain json > sc.json   # ~2 s per crate (crates.io rate limit)
  scripts/sup/publishers.py diff supply-chain/publishers.json sc.json --markdown
"""
import json
import sys


def normalise(d):
    if "crates_io_crates" in d:  # raw cargo supply-chain output
        return {c: sorted({o["login"] for o in owners}) for c, owners in sorted(d["crates_io_crates"].items())}
    return {c: sorted(v) for c, v in sorted(d.get("crates", d).items())}


def load(path):
    with open(path) as f:
        return normalise(json.load(f))


def snapshot(src, out):
    snap = load(src)
    doc = {
        "_comment": "SUP-019 publisher snapshot: crate -> crates.io owner logins. Regenerate: "
        "cargo supply-chain json > sc.json && scripts/sup/publishers.py snapshot sc.json",
        "crates": snap,
    }
    with open(out, "w") as f:
        json.dump(doc, f, indent=1, sort_keys=False)
        f.write("\n")
    owners = {o for v in snap.values() for o in v}
    print(f"snapshot: {len(snap)} crates, {len(owners)} distinct publishers -> {out}")


def diff(old_p, new_p, md):
    old, new = load(old_p), load(new_p)
    changed, added, removed = [], sorted(set(new) - set(old)), sorted(set(old) - set(new))
    for c in sorted(set(old) & set(new)):
        plus, minus = sorted(set(new[c]) - set(old[c])), sorted(set(old[c]) - set(new[c]))
        if plus or minus:
            changed.append((c, plus, minus))
    out = []
    if md:
        out.append("## Crate publisher changes (SUP-019)\n")
    if not (changed or added or removed):
        out.append("No publisher changes.")
    for c, plus, minus in changed:
        out.append(f"- **{c}**: owners added {plus or '-'}, removed {minus or '-'}  <- review")
    for c in added:
        out.append(f"- new crate {c}: {', '.join(new[c])}")
    for c in removed:
        out.append(f"- crate gone: {c}")
    print("\n".join(out))
    return 10 if changed else 0


def main(a):
    if len(a) >= 2 and a[0] == "snapshot":
        out = a[a.index("-o") + 1] if "-o" in a else "supply-chain/publishers.json"
        snapshot(a[1], out)
        return 0
    if len(a) >= 3 and a[0] == "diff":
        return diff(a[1], a[2], "--markdown" in a)
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
