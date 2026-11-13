#!/usr/bin/env python3
"""Evaluate a searchdump-rs release build on a Linux host.

See scripts/perf/RELEASE-EVAL.md. This module is the driver behind
scripts/perf/release-eval.sh: download (or accept) release binaries, isolate
podman, run the usual engine set through run-suite.sh, and write results under
scripts/perf/results/release-eval/<version>/<hostid>/ that can be committed
back from the checkout.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
import suite as S  # noqa: E402

SCHEMA = "searchdump-release-eval/v1"
DEFAULT_REPO = "robertgoley/searchdump-rs"
NODE_VERSION = "6.125.1"
PRODUCT_BINS = ("searchdump", "searchdump-multi")
HARNESS_BINS = ("edgen", "edverify")
ALL_BINS = PRODUCT_BINS + HARNESS_BINS
PRESETS = {
    "all": list(S.ENGINE_ORDER),
    "quick": ["es819-plain", "os3-plain"],
    "plain": [e for e in S.ENGINE_ORDER if e.endswith("-plain")],
    "secure": [e for e in S.ENGINE_ORDER if e.endswith("-secure")],
}
DEFAULT_REPS = 3
DEFAULT_PLACE = "box"
DEFAULT_TIER = "L0"
PORT_FIRST, PORT_LAST = 19242, 19261
MIN_DISK_GB = 8
HOST_ID_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}")
SECRET_KEY_RE = re.compile(
    r"(password|passwd|secret|token|authorization|api[_-]?key|credential|private[_-]?key)",
    re.I,
)
# Metadata endpoints are hardcoded link-local / well-known hosts only.
CLOUD_TIMEOUT_S = 0.6
USER_AGENT = "searchdump-rs-release-eval"

# ---------------------------------------------------------------------------
# Small helpers
# ---------------------------------------------------------------------------


def now() -> str:
    return datetime.datetime.now().astimezone().isoformat(timespec="seconds")


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def die(msg: str, code: int = 2) -> None:
    print(f"release-eval: {msg}", file=sys.stderr)
    raise SystemExit(code)


def log(msg: str, summary: Path | None = None) -> None:
    line = f"{datetime.datetime.now().strftime('%F %T')} {msg}"
    print(line, flush=True)
    if summary is not None:
        with summary.open("a") as f:
            f.write(line + "\n")


def write_json(path: Path, data) -> None:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2) + "\n")
    tmp.replace(path)


def run(cmd, **kw) -> subprocess.CompletedProcess:
    return subprocess.run([str(x) for x in cmd], check=True, **kw)


def capture(cmd, **kw) -> str:
    return run(cmd, capture_output=True, text=True, **kw).stdout.strip()


def which(name: str) -> str | None:
    return shutil.which(name)


def is_linux() -> bool:
    return sys.platform.startswith("linux")


def parse_github_repo(url: str) -> str | None:
    """Return owner/repo from a git remote URL, or None."""
    url = url.strip()
    url = re.sub(r"\.git$", "", url)
    m = re.search(r"github\.com[:/](?P<owner>[^/]+)/(?P<repo>[^/]+)$", url)
    if not m:
        return None
    return f"{m.group('owner')}/{m.group('repo')}"


def default_repo() -> str:
    try:
        url = capture(["git", "remote", "get-url", "origin"], cwd=ROOT)
        parsed = parse_github_repo(url)
        if parsed:
            return parsed
    except (subprocess.CalledProcessError, FileNotFoundError):
        pass
    return DEFAULT_REPO


def normalize_tag(tag: str) -> tuple[str, str]:
    """Return (tag_with_v, version_without_v). 'latest' is left as 'latest'."""
    tag = tag.strip()
    if tag == "latest" or tag == "":
        return "latest", "latest"
    version = tag[1:] if tag.startswith("v") else tag
    return "v" + version, version


def host_id_ok(value: str) -> bool:
    return bool(HOST_ID_RE.fullmatch(value))


def sanitize_host_id(raw: str) -> str:
    s = re.sub(r"[^A-Za-z0-9_.-]+", "-", raw).strip(".-")
    if not s or not s[0].isalnum():
        s = "host-" + s
    s = s[:128]
    if not host_id_ok(s):
        s = "host"
    return s


def detect_libc() -> str:
    """Return 'gnu' or 'musl'. Prefer gnu on glibc hosts (release default)."""
    try:
        p = subprocess.run(["ldd", "--version"], capture_output=True, text=True)
        out = (p.stdout or "") + (p.stderr or "")
        if "musl" in out.lower():
            return "musl"
        return "gnu"
    except FileNotFoundError:
        pass
    if Path("/lib/ld-musl-x86_64.so.1").exists() or Path("/lib/ld-musl-aarch64.so.1").exists():
        return "musl"
    return "gnu"


def detect_arch() -> str:
    m = platform.machine()
    return {"amd64": "x86_64", "arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}.get(m, m)


def detect_target() -> str:
    return f"{detect_arch()}-unknown-linux-{detect_libc()}"


def asset_names(version: str, target: str) -> dict[str, str]:
    base = f"searchdump-rs-{version}-{target}.tar.gz"
    return {"archive": base, "sha256": base + ".sha256", "sums": "SHA256SUMS"}


def parse_sha256_line(text: str, filename: str) -> str | None:
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if len(parts) < 2:
            continue
        digest, name = parts[0], parts[-1].lstrip("*")
        if name == filename or name.endswith("/" + filename) or Path(name).name == filename:
            if re.fullmatch(r"[0-9a-f]{64}", digest.lower()):
                return digest.lower()
    return None


def verify_checksum(archive: Path, digest: str) -> None:
    got = sha256_file(archive)
    if got != digest.lower():
        raise ValueError(f"checksum mismatch for {archive.name}: expected {digest}, got {got}")


# ---------------------------------------------------------------------------
# Host fingerprint (no secrets)
# ---------------------------------------------------------------------------


def _read_os_release() -> dict[str, str]:
    out = {}
    p = Path("/etc/os-release")
    if not p.exists():
        return out
    for line in p.read_text(errors="replace").splitlines():
        if "=" in line and not line.startswith("#"):
            k, v = line.split("=", 1)
            out[k] = v.strip().strip('"')
    return out


def _cpu_model() -> str | None:
    p = Path("/proc/cpuinfo")
    if not p.exists():
        return platform.processor() or None
    for line in p.read_text(errors="replace").splitlines():
        if line.lower().startswith("model name") or line.lower().startswith("cpu model"):
            return line.split(":", 1)[-1].strip()
    return None


def _mem_bytes() -> int | None:
    p = Path("/proc/meminfo")
    if not p.exists():
        return None
    for line in p.read_text().splitlines():
        if line.startswith("MemTotal:"):
            return int(line.split()[1]) * 1024
    return None


def _mem_available_mb() -> int | None:
    p = Path("/proc/meminfo")
    if not p.exists():
        return None
    for line in p.read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(int(line.split()[1]) / 1024)
    return None


def _disk_info(path: Path) -> dict:
    info: dict = {"path": str(path)}
    try:
        usage = shutil.disk_usage(path)
        info["total_bytes"] = usage.total
        info["free_bytes"] = usage.free
        info["free_gb"] = int(usage.free / (1024 ** 3))
    except OSError:
        pass
    # Best-effort backing device type (rotational / nvme / virtio) via findmnt + sysfs.
    try:
        src = capture(["findmnt", "-n", "-o", "SOURCE", "--target", str(path)])
        info["source"] = src
        name = Path(src).name
        # strip partition suffix: nvme0n1p2 -> nvme0n1, sda1 -> sda, vda2 -> vda
        disk = re.sub(r"p?\d+$", "", name) if name.startswith("nvme") else re.sub(r"\d+$", "", name)
        sysblk = Path("/sys/block") / disk
        rot = sysblk / "queue" / "rotational"
        if rot.exists():
            info["rotational"] = rot.read_text().strip() == "1"
        if disk.startswith("nvme"):
            info["kind"] = "nvme"
        elif info.get("rotational"):
            info["kind"] = "hdd"
        elif sysblk.exists():
            info["kind"] = "ssd-or-virtual"
        model = sysblk / "device" / "model"
        if model.exists():
            info["model"] = model.read_text().strip()
    except (subprocess.CalledProcessError, FileNotFoundError, OSError):
        pass
    return info


def _http_text(url: str, headers: dict | None = None, timeout: float = CLOUD_TIMEOUT_S, method: str = "GET",
               body: bytes | None = None) -> str | None:
    req = urllib.request.Request(url, data=body, method=method, headers={"User-Agent": USER_AGENT, **(headers or {})})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.read().decode("utf-8", "replace").strip()
    except (urllib.error.URLError, TimeoutError, OSError, ValueError):
        return None


def _cloud_instance() -> dict | None:
    """Best-effort instance type only. Never stores tokens, user-data, or instance ids."""
    dmi_vendor = _read_text(Path("/sys/class/dmi/id/sys_vendor"))
    dmi_product = _read_text(Path("/sys/class/dmi/id/product_name"))
    # AWS IMDSv2 then IMDSv1, instance-type only.
    token = _http_text(
        "http://169.254.169.254/latest/api/token",
        headers={"X-aws-ec2-metadata-token-ttl-seconds": "60"},
        method="PUT",
    )
    aws_headers = {"X-aws-ec2-metadata-token": token} if token else {}
    itype = _http_text("http://169.254.169.254/latest/meta-data/instance-type", headers=aws_headers)
    if itype and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", itype) and "/" not in itype:
        return {"provider": "aws", "instance_type": itype}
    gce = _http_text(
        "http://metadata.google.internal/computeMetadata/v1/instance/machine-type",
        headers={"Metadata-Flavor": "Google"},
    )
    if gce:
        # projects/<n>/machineTypes/<type>
        typ = gce.rsplit("/", 1)[-1]
        if typ:
            return {"provider": "gcp", "instance_type": typ}
    azure = _http_text(
        "http://169.254.169.254/metadata/instance/compute/vmSize?api-version=2021-02-01&format=text",
        headers={"Metadata": "true"},
    )
    if azure:
        return {"provider": "azure", "instance_type": azure}
    if dmi_product or dmi_vendor:
        vendor = (dmi_vendor or "").lower()
        product = (dmi_product or "").lower()
        if "amazon" in vendor or "amazon" in product or "ec2" in product:
            return {"provider": "aws", "instance_type": dmi_product}
        if "google" in vendor or "google" in product:
            return {"provider": "gcp", "instance_type": dmi_product}
        if "microsoft" in vendor or "azure" in product:
            return {"provider": "azure", "instance_type": dmi_product}
    return None


def _read_text(path: Path) -> str | None:
    try:
        return path.read_text(errors="replace").strip() or None
    except OSError:
        return None


def _podman_version(podman_bin: str | None = None) -> str | None:
    bin_ = podman_bin or which("podman")
    if not bin_:
        return None
    try:
        return capture([bin_, "--version"])
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def host_fingerprint(host_id: str | None = None, podman_bin: str | None = None) -> dict:
    osr = _read_os_release()
    ram = _mem_bytes()
    cores = os.cpu_count()
    hostname = platform.node() or "host"
    fp = {
        "hostname": hostname,
        "arch": detect_arch(),
        "cpu_model": _cpu_model(),
        "cpu_cores": cores,
        "ram_bytes": ram,
        "ram_gb": round(ram / (1024 ** 3), 1) if ram else None,
        "kernel": platform.release(),
        "distro": osr.get("PRETTY_NAME") or osr.get("NAME"),
        "distro_id": osr.get("ID"),
        "distro_version": osr.get("VERSION_ID"),
        "podman_version": _podman_version(podman_bin),
        "disk": _disk_info(Path("/")),
        "cloud": _cloud_instance(),
        "collected_at": now(),
        "platform": platform.platform(),
    }
    if host_id:
        if not host_id_ok(host_id):
            raise ValueError("host id must be a simple alphanumeric/dot/dash/underscore name (1-128 chars)")
        fp["host_id"] = host_id
    else:
        ram_gb = int(ram / (1024 ** 3)) if ram else 0
        raw = f"{hostname}-{fp['arch']}-{cores or 0}c-{ram_gb}g"
        fp["host_id"] = sanitize_host_id(raw)
    # Guard: never persist secret-looking keys (defence in depth).
    return strip_secrets(fp)


def strip_secrets(obj):
    """Drop dict keys whose names look like credentials. Used on anything written to results."""
    if isinstance(obj, dict):
        return {k: strip_secrets(v) for k, v in obj.items() if not SECRET_KEY_RE.search(str(k))}
    if isinstance(obj, list):
        return [strip_secrets(v) for v in obj]
    return obj


def results_dir(root: Path, version: str, host_id: str, base: Path | None = None) -> Path:
    base = base or (root / "scripts/perf/results/release-eval")
    return base / version / host_id


def reaper_patterns(run_dir: Path) -> dict[str, str]:
    """pgrep/pkill -f patterns limited to THIS isolated store / runtime dir."""
    # Paths are literal; escape regex metacharacters so a weird run dir cannot broaden the match.
    root = re.escape(str((run_dir / "podman-root").resolve()))
    xdg = re.escape(str((run_dir / "xdg-runtime").resolve()))
    return {
        "conmon": f"conmon .*{root}",
        "pasta": f"pasta .*{xdg}",
    }


# ---------------------------------------------------------------------------
# GitHub release resolution and download
# ---------------------------------------------------------------------------


def _gh_available() -> bool:
    return which("gh") is not None


def _auth_headers() -> dict:
    # Use a token only for the GitHub API, and never write it anywhere.
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    headers = {"User-Agent": USER_AGENT, "Accept": "application/vnd.github+json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    return headers


def github_api(repo: str, path: str) -> object:
    url = f"https://api.github.com/repos/{repo}{path}"
    req = urllib.request.Request(url, headers=_auth_headers())
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")
        raise ValueError(f"GitHub API {e.code} for {path}: {body[:300]}") from e
    except urllib.error.URLError as e:
        raise ValueError(f"GitHub API request failed: {e}") from e


def pick_latest_release(releases: list[dict]) -> dict:
    published = [r for r in releases if not r.get("draft")]
    if not published:
        raise ValueError(
            "no published GitHub releases found (drafts are invisible without auth; "
            "pass --tag after the release is published, or use --bin)"
        )
    stable = [r for r in published if not r.get("prerelease")]
    return stable[0] if stable else published[0]


def resolve_release(repo: str, tag: str) -> dict:
    """Return {tag, version, name, prerelease, assets: [{name, url, size}]}."""
    if tag == "latest":
        if _gh_available():
            try:
                data = json.loads(capture(
                    ["gh", "release", "list", "--repo", repo, "--limit", "30",
                     "--json", "tagName,isDraft,isPrerelease,isLatest,name"]))
                rows = []
                for r in data:
                    rows.append({
                        "tag_name": r["tagName"],
                        "draft": r.get("isDraft"),
                        "prerelease": r.get("isPrerelease"),
                        "name": r.get("name") or r["tagName"],
                    })
                # Prefer the release gh marks as latest when present; else first published stable.
                marked = next((r for r in data if r.get("isLatest") and not r.get("isDraft")), None)
                if marked:
                    chosen_tag = marked["tagName"]
                else:
                    chosen_tag = pick_latest_release(rows)["tag_name"]
                return resolve_release(repo, chosen_tag)
            except (subprocess.CalledProcessError, ValueError, json.JSONDecodeError):
                pass
        try:
            rel = github_api(repo, "/releases/latest")
        except ValueError:
            rels = github_api(repo, "/releases?per_page=30")
            if not isinstance(rels, list):
                raise ValueError("unexpected GitHub releases response")
            rel = pick_latest_release(rels)
        tag = rel["tag_name"]
    else:
        try:
            rel = github_api(repo, f"/releases/tags/{tag}")
        except ValueError as e:
            raise ValueError(
                f"could not resolve release {tag} on {repo}: {e}. "
                "Draft releases are not downloadable anonymously; publish the release or pass --bin."
            ) from e
    t, version = normalize_tag(rel["tag_name"])
    assets = []
    for a in rel.get("assets") or []:
        assets.append({
            "name": a.get("name"),
            "url": a.get("browser_download_url"),
            "size": a.get("size"),
        })
    return {
        "tag": t,
        "version": version,
        "name": rel.get("name") or t,
        "prerelease": bool(rel.get("prerelease")),
        "draft": bool(rel.get("draft")),
        "assets": assets,
    }


def download_file(url: str, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    if which("curl"):
        run(["curl", "-fsSL", "--retry", "3", "-o", str(dest), url])
        return
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=120) as resp, dest.open("wb") as f:
        shutil.copyfileobj(resp, f)


def download_release_archive(repo: str, release: dict, target: str, dest_dir: Path,
                             allow_unverified: bool = False) -> dict:
    names = asset_names(release["version"], target)
    assets = {a["name"]: a for a in release["assets"] if a.get("name")}
    info = assets.get(names["archive"])
    if not info or not info.get("url"):
        available = ", ".join(sorted(assets)) or "(none)"
        raise ValueError(
            f"release {release['tag']} has no asset {names['archive']}. Available: {available}"
        )
    dest_dir.mkdir(parents=True, exist_ok=True)
    archive = dest_dir / names["archive"]
    download_file(info["url"], archive)
    digest = None
    sha_asset = assets.get(names["sha256"])
    sums_asset = assets.get(names["sums"])
    if sha_asset and sha_asset.get("url"):
        sha_path = dest_dir / names["sha256"]
        download_file(sha_asset["url"], sha_path)
        digest = parse_sha256_line(sha_path.read_text(), names["archive"]) or parse_sha256_line(
            sha_path.read_text(), "")
        # A lone-hash .sha256 file (just the digest) is also accepted.
        if digest is None:
            text = sha_path.read_text().strip().split()
            if text and re.fullmatch(r"[0-9a-f]{64}", text[0].lower()):
                digest = text[0].lower()
    elif sums_asset and sums_asset.get("url"):
        sums_path = dest_dir / names["sums"]
        download_file(sums_asset["url"], sums_path)
        digest = parse_sha256_line(sums_path.read_text(), names["archive"])
    if digest:
        verify_checksum(archive, digest)
    elif not allow_unverified:
        archive.unlink(missing_ok=True)
        raise ValueError(
            f"no checksum file for {names['archive']}; pass --allow-unverified to accept it anyway"
        )
    extract_dir = dest_dir / "extracted"
    if extract_dir.exists():
        shutil.rmtree(extract_dir)
    extract_dir.mkdir()
    run(["tar", "-C", str(extract_dir), "-xzf", str(archive)])
    bin_dir = find_bin_dir(extract_dir, required=PRODUCT_BINS)
    return {
        "archive": str(archive),
        "sha256": digest or sha256_file(archive),
        "asset": names["archive"],
        "bin_dir": str(bin_dir),
        "target": target,
        "verified": bool(digest),
    }


def find_bin_dir(root: Path, required=PRODUCT_BINS) -> Path:
    """Find a directory under root that contains the required executables."""
    if all(os.access(root / b, os.X_OK) for b in required):
        return root
    for p in sorted(root.rglob("*")):
        if p.is_dir() and all(os.access(p / b, os.X_OK) for b in required):
            return p
    raise ValueError(f"no directory under {root} contains {', '.join(required)}")


# ---------------------------------------------------------------------------
# Preflight / plan
# ---------------------------------------------------------------------------


def select_engines(preset: str | None, engines: str | None) -> list[str]:
    if preset and engines:
        raise ValueError("pass either --preset or --engines, not both")
    if engines:
        return S.select(engines, S.ENGINE_ORDER, "engines")
    name = preset or "all"
    if name not in PRESETS:
        raise ValueError(f"unknown preset {name!r}; choose one of {', '.join(PRESETS)}")
    return list(PRESETS[name])


def image_pins() -> list[str]:
    pins = []
    for line in (ROOT / "scripts/it/images.lock").read_text().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        tag, digest = line.split()
        pins.append(f"{tag}@{digest}")
    return pins


def engine_need_mem_mb(engine: str, heap_mb: int, tool_mem_mb: int) -> int:
    heap = S.engine_heap_mb(engine, heap_mb)
    mult = 1.5 if heap >= 2048 else 2
    return int(heap * mult) + 512 + tool_mem_mb + 256


def first_free_port(start: int = PORT_FIRST, end: int = PORT_LAST, explicit: int | None = None) -> int:
    if explicit is not None:
        if S.port_in_use(explicit):
            raise ValueError(f"port {explicit} is already in use")
        return explicit
    env = os.environ.get("PERF_ENGINE_PORT")
    if env:
        if not env.isdigit() or not 1 <= int(env) <= 65535:
            raise ValueError(f"PERF_ENGINE_PORT must be a TCP port, got {env!r}")
        p = int(env)
        if S.port_in_use(p):
            raise ValueError(f"port {p} (PERF_ENGINE_PORT) is already in use")
        return p
    for p in range(start, end + 1):
        if not S.port_in_use(p):
            return p
    raise ValueError(f"no free loopback port in {start}-{end}; pass --port")


def default_run_dir() -> Path:
    cache = Path(os.environ.get("XDG_CACHE_HOME") or (Path.home() / ".cache"))
    return cache / "searchdump-rs" / "release-eval"


def build_plan(a: argparse.Namespace) -> dict:
    tag_arg = a.tag
    version = "local"
    tag = tag_arg
    release = None
    if not a.bin:
        t, version = normalize_tag(tag_arg)
        tag = t
        if a.dry_run and t == "latest":
            # Dry-run may not hit the network: leave a placeholder the runner resolves later.
            release = None
        else:
            try:
                release = resolve_release(a.repo, t)
                tag, version = release["tag"], release["version"]
            except ValueError as e:
                if a.dry_run:
                    release = {"error": str(e), "tag": t, "version": version}
                else:
                    raise
    else:
        t, v = normalize_tag(tag_arg) if tag_arg != "latest" else ("local", "local")
        if tag_arg != "latest":
            tag, version = t, v
        else:
            tag, version = "local", "local"

    engines = select_engines(a.preset, a.engines)
    cases = a.cases
    # Validate cases/place/tier through the existing planner (no containers).
    ns = argparse.Namespace(
        tier=a.tier, place=a.place, engines=",".join(engines), cases=cases,
        skip_node=not a.compare_node, reps=a.reps, threshold=10,
    )
    suite_plan = S.make_plan(ns)
    host = host_fingerprint(a.host_id)
    host_id = host["host_id"]
    target = a.target or detect_target()
    out = results_dir(ROOT, version, host_id, Path(a.results_dir) if a.results_dir else None)
    run_dir = Path(a.run_dir).resolve() if a.run_dir else default_run_dir()
    port = None
    port_error = None
    try:
        port = first_free_port(explicit=a.port)
    except ValueError as e:
        port_error = str(e)
        if not a.dry_run:
            raise
    mem_av = _mem_available_mb()
    disk = _disk_info(Path("/"))
    by_heap = {x["engine"]: x["heap_mb"] for x in suite_plan["engines"]}
    needs = [
        {
            "engine": e,
            "heap_mb": by_heap[e],
            "need_mem_mb": engine_need_mem_mb(e, by_heap[e], suite_plan["place_config"]["tool_mem_mb"]),
        }
        for e in engines
    ]

    preflight = preflight_report(
        dry_run=a.dry_run,
        mem_available_mb=mem_av,
        disk_free_gb=disk.get("free_gb"),
        needs=needs,
        port=port,
        port_error=port_error,
        run_dir=run_dir,
        bin_dir=Path(a.bin) if a.bin else None,
    )
    return {
        "schema": SCHEMA,
        "dry_run": bool(a.dry_run),
        "repo": a.repo,
        "tag": tag,
        "version": version,
        "release": release,
        "target": target,
        "bin": str(Path(a.bin).resolve()) if a.bin else None,
        "compare_node": bool(a.compare_node),
        "node_version": NODE_VERSION if a.compare_node else None,
        "preset": a.preset,
        "engines": engines,
        "cases": cases,
        "tier": a.tier,
        "place": a.place,
        "reps": a.reps,
        "host_id": host_id,
        "host": host,
        "results_dir": str(out),
        "run_dir": str(run_dir),
        "port": port,
        "port_error": port_error,
        "reaper": reaper_patterns(run_dir),
        "image_pins": image_pins(),
        "need_disk_mb": suite_plan["need_disk_mb"],
        "engine_needs": needs,
        "suite_engine_order": suite_plan["engine_order"],
        "preflight": preflight,
        "commit": bool(a.commit or a.push),
        "push": bool(a.push),
        "update_baselines": bool(a.update_baselines),
        "skip_pull": bool(a.skip_pull),
        "allow_unverified": bool(a.allow_unverified),
        "notes": [
            "Engines use --network=host via the existing IT/perf helpers; do not combine host networking with --userns=keep-id.",
            "The podman store and XDG_RUNTIME_DIR live under run_dir; the host's default store is never used.",
            "Secure engines get throwaway passwords from scripts/it/up.sh; they are not written to results.",
            "Release archives contain searchdump and searchdump-multi; edgen and edverify (seed/verify helpers) come from this checkout if the archive does not include them.",
        ],
    }


def preflight_report(*, dry_run: bool, mem_available_mb, disk_free_gb, needs, port, port_error,
                     run_dir: Path, bin_dir: Path | None) -> dict:
    errors = []
    warnings = []
    if not is_linux():
        errors.append(f"Linux host required (this is {sys.platform})")
    if not which("podman"):
        errors.append("podman is required (rootless is fine; no sudo)")
    for cmd in ("bash", "curl", "tar", "sha256sum", "git"):
        if not which(cmd):
            errors.append(f"missing prerequisite {cmd}")
    if sys.version_info < (3, 11):
        errors.append("Python 3.11+ is required")
    if disk_free_gb is not None and disk_free_gb < MIN_DISK_GB:
        errors.append(f"disk: {disk_free_gb}G free, need at least {MIN_DISK_GB}G")
    if mem_available_mb is not None and needs:
        hardest = max(n["need_mem_mb"] for n in needs)
        if mem_available_mb < hardest:
            warnings.append(
                f"MemAvailable {mem_available_mb} MB is below the heaviest selected engine estimate "
                f"({hardest} MB, usually OpenSearch 2/3). The suite will wait or refuse that engine."
            )
    if port_error:
        (warnings if dry_run else errors).append(port_error)
    if bin_dir is not None:
        missing = [b for b in PRODUCT_BINS if not os.access(bin_dir / b, os.X_OK)]
        if missing and not dry_run:
            errors.append(f"--bin {bin_dir} is missing executable {', '.join(missing)}")
        elif missing:
            warnings.append(f"--bin {bin_dir} is missing executable {', '.join(missing)}")
    cores = os.cpu_count() or 0
    if cores and cores < 2:
        warnings.append(f"only {cores} CPU core visible; runs will work but will be slow")
    return {
        "ok": not errors,
        "errors": errors,
        "warnings": warnings,
        "podman": which("podman"),
        "mem_available_mb": mem_available_mb,
        "disk_free_gb": disk_free_gb,
        "port": port,
        "run_dir": str(run_dir),
        "linux": is_linux(),
    }


# ---------------------------------------------------------------------------
# Isolation, harvest, commit
# ---------------------------------------------------------------------------


def write_podman_wrapper(run_dir: Path, real_podman: str) -> Path:
    run_dir = run_dir.resolve()
    root = run_dir / "podman-root"
    runroot = run_dir / "podman-run"
    xdg = run_dir / "xdg-runtime"
    bindir = run_dir / "bin"
    for d in (root, runroot, xdg, bindir, run_dir / "steps", run_dir / "logs", run_dir / "bins"):
        d.mkdir(parents=True, exist_ok=True)
    os.chmod(xdg, 0o700)
    wrapper = bindir / "podman"
    wrapper.write_text(
        "#!/bin/sh\n"
        f"# Isolated store for this release-eval run dir; never the user's default store.\n"
        f'exec "{real_podman}" --root "{root}" --runroot "{runroot}" "$@"\n'
    )
    os.chmod(wrapper, 0o755)
    return wrapper


def reap_orphans(run_dir: Path) -> None:
    patterns = reaper_patterns(run_dir)
    try:
        conmons = capture(["pgrep", "-f", patterns["conmon"]]).split()
    except subprocess.CalledProcessError:
        conmons = []
    for pid in conmons:
        subprocess.run(["pkill", "-KILL", "-P", pid], check=False, capture_output=True)
        subprocess.run(["kill", "-KILL", pid], check=False, capture_output=True)
    subprocess.run(["pkill", "-KILL", "-f", patterns["pasta"]], check=False, capture_output=True)


def cleanup_isolated_store(env: dict) -> None:
    """Remove leftover perf containers/volumes in the *isolated* store only."""
    try:
        names = capture(["podman", "ps", "-a", "--format", "{{.Names}}"], env=env)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return
    for name in names.splitlines():
        if re.search(r"-perf-[0-9]+-", name):
            subprocess.run(["podman", "rm", "-f", name], env=env, check=False, capture_output=True)
    subprocess.run(["podman", "volume", "prune", "-f"], env=env, check=False, capture_output=True)


def isolated_env(run_dir: Path, real_podman: str, extra: dict | None = None) -> dict:
    write_podman_wrapper(run_dir, real_podman)
    env = dict(os.environ)
    env["PATH"] = str(run_dir / "bin") + ":" + env.get("PATH", "")
    env["XDG_RUNTIME_DIR"] = str(run_dir / "xdg-runtime")
    if extra:
        env.update({k: str(v) for k, v in extra.items() if v is not None})
    return env


def pull_pins(env: dict, pins: list[str], log_path: Path) -> None:
    with log_path.open("a") as f:
        for pin in pins:
            f.write(f"+ podman pull {pin}\n")
            f.flush()
            p = subprocess.run(["podman", "pull", pin], env=env, stdout=f, stderr=subprocess.STDOUT)
            if p.returncode != 0:
                raise ValueError(f"podman pull failed for {pin} (see {log_path})")


def prepare_binaries(plan: dict, run_dir: Path, a: argparse.Namespace) -> dict:
    dest = run_dir / "bins"
    dest.mkdir(parents=True, exist_ok=True)
    meta = {"source": "local" if a.bin else "github-release", "bin_dir": str(dest)}
    if a.bin:
        src = Path(a.bin).resolve()
        for b in PRODUCT_BINS:
            if not os.access(src / b, os.X_OK):
                raise ValueError(f"--bin {src} is missing executable {b}")
            shutil.copy2(src / b, dest / b)
            os.chmod(dest / b, 0o755)
        for b in HARNESS_BINS:
            if os.access(src / b, os.X_OK):
                shutil.copy2(src / b, dest / b)
                os.chmod(dest / b, 0o755)
        meta["provided"] = str(src)
    else:
        release = plan["release"]
        if not release or release.get("error"):
            raise ValueError(release.get("error") if release else "could not resolve the GitHub release")
        dl = download_release_archive(
            plan["repo"], release, plan["target"], run_dir / "download",
            allow_unverified=a.allow_unverified,
        )
        src = Path(dl["bin_dir"])
        for b in PRODUCT_BINS:
            shutil.copy2(src / b, dest / b)
            os.chmod(dest / b, 0o755)
        for b in HARNESS_BINS:
            if os.access(src / b, os.X_OK):
                shutil.copy2(src / b, dest / b)
                os.chmod(dest / b, 0o755)
        meta.update({k: dl[k] for k in ("archive", "sha256", "asset", "target", "verified")})
    missing_harness = [b for b in HARNESS_BINS if not os.access(dest / b, os.X_OK)]
    if missing_harness:
        built = build_harness_tools(dest, missing_harness)
        meta["harness_built_from_checkout"] = built
    missing = [b for b in ALL_BINS if not os.access(dest / b, os.X_OK)]
    if missing:
        raise ValueError("missing binaries after preparation: " + ", ".join(missing))
    meta["tool_hashes"] = {b: sha256_file(dest / b) for b in ALL_BINS}
    meta["tool_versions"] = {}
    for b in PRODUCT_BINS:
        try:
            meta["tool_versions"][b] = capture([dest / b, "--version"]).splitlines()[-1]
        except subprocess.CalledProcessError:
            meta["tool_versions"][b] = "unknown"
    return meta


def build_harness_tools(dest: Path, names: list[str]) -> dict:
    """Build edgen/edverify from this checkout. Never builds searchdump (the binary under test)."""
    if not which("cargo"):
        raise ValueError(
            "release archives do not include edgen/edverify (the suite's seed/verify helpers). "
            "Build them from this checkout (cargo build --release -p edgen -p edverify) or put "
            "them in --bin, and rerun."
        )
    toolchain = "1.91.1"
    rust_toolchain = ROOT / "rust-toolchain.toml"
    # rust-toolchain.toml, if present, already selects the toolchain; still pin for consistency.
    pkgs = []
    if "edgen" in names:
        pkgs += ["-p", "edgen"]
    if "edverify" in names:
        pkgs += ["-p", "edverify"]
    log(f"building harness tools from checkout: {' '.join(names)}")
    run(
        ["cargo", f"+{toolchain}", "build", "--locked", "--release", *pkgs,
         "-j", os.environ.get("PERF_BUILD_JOBS", "2")],
        cwd=ROOT,
    )
    for b in names:
        src = ROOT / "target/release" / b
        if not os.access(src, os.X_OK):
            raise ValueError(f"cargo built no executable {src}")
        shutil.copy2(src, dest / b)
        os.chmod(dest / b, 0o755)
    return {
        "tools": names,
        "git": _git_head(),
        "toolchain": toolchain,
    }


def _git_head() -> str | None:
    try:
        return capture(["git", "rev-parse", "HEAD"], cwd=ROOT)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def install_node(run_dir: Path) -> Path:
    """Install pinned Node elasticdump into the run dir (not the repo)."""
    src_pkg = HERE / "node-package.json"
    src_lock = HERE / "node-package-lock.json"
    dest = run_dir / "node-ed"
    dest.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src_pkg, dest / "package.json")
    shutil.copy2(src_lock, dest / "package-lock.json")
    npm = which("npm")
    if not npm:
        raise ValueError("--compare-node requires npm on the host to install elasticdump 6.125.1")
    run([npm, "ci", "--ignore-scripts"], cwd=dest)
    bindir = dest / "node_modules" / ".bin"
    ed = bindir / "elasticdump"
    if not os.access(ed, os.X_OK):
        raise ValueError(f"npm ci did not produce {ed}")
    ver = capture([ed, "--version"]).splitlines()[-1]
    if ver != NODE_VERSION:
        raise ValueError(f"Node elasticdump must be exactly {NODE_VERSION}, got {ver}")
    return bindir


def collect_engine_result(suite_dir: Path, dest: Path, engine: str) -> dict:
    """Copy only committed-safe artifacts (no work dirs, no auth files)."""
    engine_dest = dest / "engines" / engine
    engine_dest.mkdir(parents=True, exist_ok=True)
    copied = []
    for name in ("combined.json", "summary.md"):
        src = suite_dir / name
        if src.exists():
            data_dest = engine_dest / name
            if name.endswith(".json"):
                write_json(data_dest, strip_secrets(json.loads(src.read_text())))
            else:
                shutil.copy2(src, data_dest)
            copied.append(name)
    engine_json = suite_dir / engine / "engine.json"
    if engine_json.exists():
        write_json(engine_dest / "engine.json", strip_secrets(json.loads(engine_json.read_text())))
        copied.append("engine.json")
    # Drop bulky work copies in the *suite* output (dumps); never copy them into results.
    work = suite_dir / engine / "work"
    if work.exists():
        shutil.rmtree(work, ignore_errors=True)
    return {"copied": copied, "suite_dir": str(suite_dir)}


def write_aggregate(dest: Path, version: str, host_id: str) -> Path:
    runs = []
    engines_root = dest / "engines"
    if engines_root.exists():
        for p in sorted(engines_root.iterdir()):
            if (p / "combined.json").exists():
                runs.append(str(p))
    report = dest / "report.md"
    if not runs:
        report.write_text(f"# searchdump-rs {version} release-eval on {host_id}\n\nNo engine results yet.\n")
        return report
    title = f"searchdump-rs {version} release-eval on {host_id}"
    proc = subprocess.run(
        [sys.executable, str(HERE / "aggregate.py"), "--title", title, *runs],
        check=True, capture_output=True, text=True,
    )
    report.write_text(proc.stdout)
    return report


def commit_message(version: str, host_id: str) -> str:
    return f"perf: release-eval {version} results from {host_id}"


def git_commit_results(path: Path, message: str, push: bool = False) -> dict:
    """Stage and commit *only* this results path. Never git add -A."""
    path = path.resolve()
    if not path.is_relative_to(ROOT.resolve()):
        raise ValueError("results path must be inside the repository")
    rel = path.relative_to(ROOT.resolve())
    run(["git", "add", "--", str(rel)], cwd=ROOT)
    # --only so an unrelated staged file (another worker, a leftover) is not committed.
    try:
        run(["git", "commit", "--only", "-q", "-m", message, "--", str(rel)], cwd=ROOT)
        committed = True
    except subprocess.CalledProcessError as e:
        # Nothing to commit is not a hard failure.
        staged = capture(["git", "status", "--short", "--", str(rel)], cwd=ROOT)
        if not staged:
            committed = False
        else:
            raise ValueError(f"git commit failed: {e}") from e
    branch = capture(["git", "rev-parse", "--abbrev-ref", "HEAD"], cwd=ROOT)
    sha = capture(["git", "rev-parse", "HEAD"], cwd=ROOT)
    push_cmd = ["git", "push", "origin", f"HEAD:{branch}"]
    pushed = False
    if push and committed:
        run(push_cmd, cwd=ROOT)
        pushed = True
    return {
        "committed": committed,
        "sha": sha if committed else None,
        "message": message,
        "path": str(rel),
        "push_command": " ".join(push_cmd),
        "pushed": pushed,
    }


# ---------------------------------------------------------------------------
# CLI / main loop
# ---------------------------------------------------------------------------


def build_arg_parser() -> argparse.ArgumentParser:
    epilog = """\
Examples:
  scripts/perf/release-eval.sh --dry-run
  scripts/perf/release-eval.sh --preset quick --tag v1.0.0-beta1
  scripts/perf/release-eval.sh --tag v1.0.0-beta1 --compare-node --commit
  scripts/perf/release-eval.sh --bin /path/to/bins --tag local --preset quick
"""
    ap = argparse.ArgumentParser(
        prog="scripts/perf/release-eval.sh",
        description="Evaluate a searchdump-rs release on a Linux host (isolated podman, usual engines).",
        epilog=epilog,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("--tag", "--version", dest="tag", default="latest",
                    help="GitHub release tag (default: latest published, including a pre-release if that is all there is)")
    ap.add_argument("--repo", default=None, help=f"owner/repo (default: origin or {DEFAULT_REPO})")
    ap.add_argument("--bin", metavar="DIR", help="local directory with searchdump and searchdump-multi (skips GitHub)")
    ap.add_argument("--target", help="archive rustc target (default: auto, prefer *-linux-gnu on glibc hosts)")
    ap.add_argument("--allow-unverified", action="store_true",
                    help="accept a downloaded archive that has no checksum file")
    ap.add_argument("--compare-node", action="store_true",
                    help=f"also measure Node elasticdump {NODE_VERSION} (opt-in; installed into the run dir)")
    ap.add_argument("--engines", help="comma-separated engine ids (default: the full 16-engine set)")
    ap.add_argument("--preset", choices=sorted(PRESETS),
                    help="engine preset: all, quick (es819-plain,os3-plain), plain, secure")
    ap.add_argument("--cases", help="comma-separated case ids (default: the suite's full case list)")
    ap.add_argument("--tier", choices=("L0", "L1"), default=DEFAULT_TIER)
    ap.add_argument("--place", default=DEFAULT_PLACE, help=f"suite place (default: {DEFAULT_PLACE})")
    ap.add_argument("--reps", type=int, default=DEFAULT_REPS)
    ap.add_argument("--host-id", dest="host_id", help="override the generated host id")
    ap.add_argument("--run-dir", help="isolated state/store dir (default: $XDG_CACHE_HOME/searchdump-rs/release-eval)")
    ap.add_argument("--port", type=int, help=f"engine loopback port (default: first free in {PORT_FIRST}-{PORT_LAST})")
    ap.add_argument("--results-dir", help="override results root (default: scripts/perf/results/release-eval)")
    ap.add_argument("--commit", action="store_true",
                    help="git add and commit only the results dir (does not push)")
    ap.add_argument("--push", action="store_true", help="like --commit, then git push (opt-in)")
    ap.add_argument("--update-baselines", action="store_true",
                    help="also write suite baselines under the results dir")
    ap.add_argument("--skip-pull", action="store_true",
                    help="do not podman pull; images must already exist in the isolated store")
    ap.add_argument("--dry-run", action="store_true",
                    help="print the plan and preflight; do not download, pull, or start containers")
    return ap


def parse_args(argv=None) -> argparse.Namespace:
    ap = build_arg_parser()
    a = ap.parse_args(argv)
    if a.reps < 1:
        ap.error("--reps must be positive")
    if a.port is not None and not 1 <= a.port <= 65535:
        ap.error("--port must be a TCP port")
    if a.repo is None:
        a.repo = default_repo()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", a.repo):
        ap.error("--repo must be owner/repo")
    return a


def format_plan(plan: dict) -> str:
    lines = [
        f"# searchdump-rs release-eval plan ({SCHEMA})",
        f"version: {plan['version']}  tag: {plan['tag']}  repo: {plan['repo']}",
        f"binaries: {'--bin ' + plan['bin'] if plan['bin'] else 'GitHub asset for ' + plan['target']}",
        f"engines ({len(plan['engines'])}): {', '.join(plan['engines'])}",
        f"tier={plan['tier']} place={plan['place']} reps={plan['reps']} "
        f"compare_node={plan['compare_node']}",
        f"host_id: {plan['host_id']}",
        f"results: {plan['results_dir']}",
        f"run_dir: {plan['run_dir']}",
        f"port: {plan['port'] or plan.get('port_error')}",
    ]
    pf = plan["preflight"]
    if pf["errors"]:
        lines.append("preflight errors:")
        lines.extend(f"  - {e}" for e in pf["errors"])
    if pf["warnings"]:
        lines.append("preflight warnings:")
        lines.extend(f"  - {w}" for w in pf["warnings"])
    if pf["ok"]:
        lines.append("preflight: ok")
    lines.append("notes:")
    lines.extend(f"  - {n}" for n in plan["notes"])
    return "\n".join(lines) + "\n"


def _suite_out_dir(log_text: str) -> Path | None:
    matches = re.findall(r"(/[^\s]*?/results/perf/[^\s/]+)", log_text)
    return Path(matches[-1]) if matches else None


def run_eval(a: argparse.Namespace) -> int:
    plan = build_plan(a)
    if a.dry_run:
        print(format_plan(plan), end="")
        print("---")
        print(json.dumps(plan, indent=2))
        return 0 if plan["preflight"]["ok"] else 2

    if not plan["preflight"]["ok"]:
        print(format_plan(plan), end="", file=sys.stderr)
        die("preflight failed; see errors above")

    run_dir = Path(plan["run_dir"])
    run_dir.mkdir(parents=True, exist_ok=True)
    summary = run_dir / "summary.txt"
    dest = Path(plan["results_dir"])
    dest.mkdir(parents=True, exist_ok=True)
    write_json(dest / "host-metadata.json", strip_secrets(plan["host"]))
    write_json(dest / "plan.json", strip_secrets({k: plan[k] for k in plan if k != "host"}))

    real_podman = which("podman")
    if not real_podman:
        die("podman not found")
    env = isolated_env(run_dir, real_podman, {
        "PERF_HOST_ID": plan["host_id"],
        "PERF_ENGINE_PORT": plan["port"],
        "LOAD_PLACE": plan["place"],
        "PERF_BIN_DIR": str(run_dir / "bins"),
    })

    def on_exit(_signum=None, _frame=None):
        try:
            cleanup_isolated_store(env)
        except Exception:
            pass

    signal.signal(signal.SIGTERM, lambda s, f: (_ for _ in ()).throw(KeyboardInterrupt(f"signal {s}")))

    try:
        bins_meta = prepare_binaries(plan, run_dir, a)
        env["PERF_BIN_DIR"] = bins_meta["bin_dir"]
        if a.compare_node:
            node_bin = install_node(run_dir)
            env["NODE_ED_BIN"] = str(node_bin)
        if not a.skip_pull:
            log("pulling pinned engine/helper images into the isolated store", summary)
            pull_pins(env, plan["image_pins"], run_dir / "logs" / "pull.log")

        write_json(dest / "manifest.json", strip_secrets({
            "schema": SCHEMA,
            "version": plan["version"],
            "tag": plan["tag"],
            "repo": plan["repo"],
            "host_id": plan["host_id"],
            "started": now(),
            "binaries": bins_meta,
            "harness_git": _git_head(),
            "compare_node": plan["compare_node"],
            "node_version": plan["node_version"],
            "tier": plan["tier"],
            "place": plan["place"],
            "reps": plan["reps"],
            "engines": plan["engines"],
            "cases": plan["cases"],
            "port": plan["port"],
            "target": plan["target"],
        }))

        Path("/tmp/searchdump-it.lock").touch()
        baseline_dir = dest / "baselines"
        if a.update_baselines:
            baseline_dir.mkdir(exist_ok=True)

        for engine in plan["engines"]:
            mark = run_dir / "steps" / f"{engine}.done"
            if mark.exists():
                log(f"skip {engine} (already done)", summary)
                continue
            if (run_dir / "STOP").exists():
                log("STOP file present; leaving remaining engines for a later resume", summary)
                break
            free_gb = _disk_info(Path("/")).get("free_gb")
            if free_gb is not None and free_gb < MIN_DISK_GB:
                die(f"disk: {free_gb}G free before {engine}", 3)
            cleanup_isolated_store(env)
            reap_orphans(run_dir)
            time.sleep(2)
            if S.port_in_use(plan["port"]):
                die(f"port {plan['port']} busy before {engine}", 5)
            n = 0
            need = next(x["need_mem_mb"] for x in plan["engine_needs"] if x["engine"] == engine)
            until = _mem_available_mb() or 0
            while until < min(need, 4500) and n <= 60:
                time.sleep(10)
                n += 1
                until = _mem_available_mb() or 0
            log(f"START {engine} mem={until}M", summary)
            t0 = time.monotonic()
            lf = run_dir / "logs" / f"{engine}.log"
            cmd = [
                "bash", str(HERE / "run-suite.sh"), "--host-tools",
                "--tier", plan["tier"], "--place", plan["place"],
                "--reps", str(plan["reps"]), "--engines", engine,
            ]
            if plan["cases"]:
                cmd += ["--cases", plan["cases"]]
            if not plan["compare_node"]:
                cmd += ["--skip-node"]
            if a.update_baselines:
                cmd += ["--baseline-dir", str(baseline_dir), "--update-baselines"]
            else:
                # Keep baselines inside the results tree so a stray official baseline is not overwritten.
                (dest / "baselines-readonly").mkdir(exist_ok=True)
                cmd += ["--baseline-dir", str(dest / "baselines-readonly")]
            with lf.open("w") as logf:
                proc = subprocess.run(cmd, env=env, cwd=ROOT, stdout=logf, stderr=subprocess.STDOUT)
            rc = proc.returncode
            elapsed = time.monotonic() - t0
            text = lf.read_text(errors="replace") if lf.exists() else ""
            suite_dir = _suite_out_dir(text)
            log(f"{engine} rc={rc} {int(elapsed // 60)}min", summary)
            if rc != 0 and elapsed < 30:
                die(f"fast failure at {engine} (log: {lf})", 4)
            if suite_dir is None or not (suite_dir / "combined.json").exists():
                log(f"NO-RESULT {engine} (not marked done)", summary)
                continue
            collect_engine_result(suite_dir, dest, engine)
            mark.write_text(f"rc={rc} dir={suite_dir}\n")
            write_aggregate(dest, plan["version"], plan["host_id"])

        write_aggregate(dest, plan["version"], plan["host_id"])
        manifest_path = dest / "manifest.json"
        manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
        manifest["finished"] = now()
        write_json(manifest_path, strip_secrets(manifest))
        log(f"results: {dest}", summary)
        print(f"RELEASE-EVAL results at {dest}")
        print(f"Report: {dest / 'report.md'}")
        if a.commit or a.push:
            info = git_commit_results(dest, commit_message(plan["version"], plan["host_id"]), push=a.push)
            if info["committed"]:
                print(f"Committed {info['sha'][:12]} {info['message']}")
            else:
                print("Nothing new to commit in the results dir.")
            if not info["pushed"]:
                print(f"To publish: {info['push_command']}")
        else:
            rel = Path(plan["results_dir"])
            try:
                rel = rel.resolve().relative_to(ROOT.resolve())
            except ValueError:
                pass
            print(f"To record these results in git:")
            print(f"  git add -- {rel}")
            print(f"  git commit --only -m '{commit_message(plan['version'], plan['host_id'])}' -- {rel}")
            branch = "HEAD"
            try:
                branch = capture(["git", "rev-parse", "--abbrev-ref", "HEAD"], cwd=ROOT)
            except Exception:
                pass
            print(f"  git push origin HEAD:{branch}")
        return 0
    except KeyboardInterrupt:
        log("interrupted; isolated store left in place for resume (re-run the same command)", summary)
        return 130
    finally:
        on_exit()


def main(argv=None) -> int:
    a = parse_args(argv)
    try:
        return run_eval(a)
    except ValueError as e:
        print("release-eval: " + str(e), file=sys.stderr)
        return 2


if __name__ == "__main__":
    os.umask(0o077)
    sys.exit(main())
