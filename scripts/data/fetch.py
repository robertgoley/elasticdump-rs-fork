#!/usr/bin/env python3
"""Dataset cache manager (TST-INF-008, TST-DATA-004). Stdlib only.

  scripts/data/fetch.sh --list
  scripts/data/fetch.sh --verify [NAME...]       # manifest + licence gate + cached files
  scripts/data/fetch.sh NAME...                  # download (resume) and verify
  scripts/data/fetch.sh --max-docs N --out F NAME   # first N records, decompressed (prefix only)

Env: ED_DATA_CACHE (default /var/tmp/searchdump-data), ED_DATASETS (manifest
path, default datasets.toml at the repo root), CI=true or --ci enforces the
CI licence gate (policy.ci_license_classes).
"""
import argparse, bz2, gzip, hashlib, lzma, os, subprocess, sys, tomllib, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
REQUIRED = ["url", "size", "sha256", "license", "license_class", "tier", "format", "compression", "converter"]
CLASSES = {"ok", "ask", "blocked"}


def load(path):
    with open(path, "rb") as f:
        m = tomllib.load(f)
    return m.get("policy", {}), m.get("datasets", {})


def check_manifest(policy, ds):
    errs = []
    for name, d in ds.items():
        for k in REQUIRED:
            if k not in d:
                errs.append(f"{name}: missing {k}")
        if d.get("license_class") not in CLASSES:
            errs.append(f"{name}: license_class must be one of {sorted(CLASSES)}")
        sha = d.get("sha256", "")
        if sha and (len(sha) != 64 or any(c not in "0123456789abcdef" for c in sha)):
            errs.append(f"{name}: bad sha256")
        url = str(d.get("url", ""))
        loop_ok = os.environ.get("ED_DATA_TEST") == "1" and url.startswith("http://127.0.0.1:")
        if not url.startswith("https://") and not loop_ok:
            errs.append(f"{name}: url must be https")
    return errs


def license_gate(name, d, policy, ci):
    allowed = policy.get("ci_license_classes", ["ok"])
    cls = d.get("license_class")
    if cls == "blocked":
        return f"{name}: licence class 'blocked' ({d.get('license')}): not usable here"
    if ci and cls not in allowed:
        return f"{name}: licence class '{cls}' ({d.get('license')}) is not allowed in CI (policy.ci_license_classes={allowed})"
    return None


def cache_dir():
    d = os.environ.get("ED_DATA_CACHE", "/var/tmp/searchdump-data")
    os.makedirs(d, mode=0o755, exist_ok=True)
    return d


def fname(name, d):
    ext = {"bz2": ".bz2", "gz": ".gz", "zst": ".zst", "zip": ".zip", "xz": ".xz"}.get(d["compression"], "")
    return os.path.join(cache_dir(), f"{name}{ext}")


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def download(url, part):
    have = os.path.getsize(part) if os.path.exists(part) else 0
    req = urllib.request.Request(url, headers={"User-Agent": "searchdump-fetch/1"})
    if have:
        req.add_header("Range", f"bytes={have}-")
    with urllib.request.urlopen(req, timeout=120) as r:
        mode = "ab" if have and r.status == 206 else "wb"
        with open(part, mode) as f:
            while True:
                b = r.read(1 << 20)
                if not b:
                    break
                f.write(b)


def fetch(name, d, allow_unpinned):
    path = fname(name, d)
    sha = d["sha256"]
    if os.path.exists(path):
        if sha and sha256_file(path) == sha:
            print(f"{name}: cached {path}")
            return 0
        if not sha:
            print(f"{name}: cached (unpinned) {path} sha256={sha256_file(path)}")
            return 0
        print(f"{name}: cached file has the wrong checksum, refetching", file=sys.stderr)
        os.unlink(path)
    if not sha and not allow_unpinned:
        print(f"{name}: not pinned (sha256 empty); rerun with --allow-unpinned to fetch and print the hash", file=sys.stderr)
        return 1
    part = path + ".part"
    download(d["url"], part)
    got = sha256_file(part)
    size = os.path.getsize(part)
    if sha and got != sha:
        os.unlink(part)
        print(f"{name}: CHECKSUM MISMATCH: expected {sha} got {got} ({size} bytes)", file=sys.stderr)
        return 1
    if d.get("size") and size != d["size"]:
        print(f"{name}: warning: size {size} != manifest {d['size']}", file=sys.stderr)
        if sha:
            return 1
    os.replace(part, path)
    print(f"{name}: fetched {path} sha256={got}{'' if sha else '  (pin this in datasets.toml)'}")
    return 0


def opener(path, comp):
    if comp == "bz2":
        return bz2.open(path, "rb")
    if comp == "gz":
        return gzip.open(path, "rb")
    if comp == "xz":
        return lzma.open(path, "rb")
    if comp == "zst":
        # stdlib has no zstd before 3.14: stream through edgen (workspace tool).
        edgen = os.environ.get("EDGEN", os.path.join(ROOT, "target/release/edgen"))
        p = subprocess.Popen([edgen, "cat", path], stdout=subprocess.PIPE)
        return p.stdout
    if comp == "none":
        return open(path, "rb")
    raise SystemExit(f"cannot stream compression {comp}")


def prefix(name, d, n, out):
    """First n non-empty lines, decompressing only as much as needed."""
    path = fname(name, d)
    if not os.path.exists(path):
        raise SystemExit(f"{name}: not cached; fetch it first")
    k = 0
    with opener(path, d["compression"]) as f, open(out, "wb") as o:
        for line in f:
            if line.strip():
                o.write(line if line.endswith(b"\n") else line + b"\n")
                k += 1
                if k >= n:
                    break
    print(f"{name}: wrote first {k} records to {out}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--verify", action="store_true")
    ap.add_argument("--ci", action="store_true", default=os.environ.get("CI") == "true")
    ap.add_argument("--allow-unpinned", action="store_true")
    ap.add_argument("--max-docs", type=int)
    ap.add_argument("--out")
    a = ap.parse_args()
    manifest = os.environ.get("ED_DATASETS", os.path.join(ROOT, "datasets.toml"))
    policy, ds = load(manifest)
    errs = check_manifest(policy, ds)
    if errs:
        print("\n".join("manifest: " + e for e in errs), file=sys.stderr)
        return 1
    if a.list:
        for n, d in ds.items():
            pin = "pinned" if d["sha256"] else "UNPINNED"
            print(f"{n:28} {d['tier']:3} {d['license_class']:8} {pin:8} {d['size']:>12}  {d['license']}")
        return 0
    names = a.names or (list(ds) if a.verify else [])
    unknown = [n for n in names if n not in ds]
    if unknown:
        print(f"unknown dataset(s): {', '.join(unknown)}", file=sys.stderr)
        return 1
    rc = 0
    for n in names:
        g = license_gate(n, ds[n], policy, a.ci)
        if g:
            # --verify over the whole manifest only reports blocked entries.
            if a.verify and not a.names and ds[n]["license_class"] != "ok":
                print(f"{n}: skipped by licence gate ({ds[n]['license_class']})")
                continue
            print("LICENCE GATE: " + g, file=sys.stderr)
            rc = 1
            continue
        if a.verify:
            p = fname(n, ds[n])
            if not os.path.exists(p):
                print(f"{n}: not cached (manifest ok)")
            elif not ds[n]["sha256"]:
                print(f"{n}: cached, unpinned sha256={sha256_file(p)}")
            elif sha256_file(p) == ds[n]["sha256"]:
                print(f"{n}: cached, checksum ok")
            else:
                print(f"{n}: CHECKSUM MISMATCH in cache {p}", file=sys.stderr)
                rc = 1
        elif a.max_docs:
            if not a.out:
                raise SystemExit("--max-docs needs --out")
            prefix(n, ds[n], a.max_docs, a.out)
        else:
            rc |= fetch(n, ds[n], a.allow_unpinned)
    if a.verify and rc == 0:
        print(f"verify: OK ({len(ds)} entries)")
    return rc


if __name__ == "__main__":
    sys.exit(main())
