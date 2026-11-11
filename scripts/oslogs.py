#!/usr/bin/env python3
"""Backup and restore one OpenSearch index with searchdump.

searchdump runs in podman on the host network. When the cluster uses
basic auth, credentials go in an env file, not on the process command
line. Cluster calls and the settings cleanup use the Python standard library.
"""

from __future__ import annotations

import base64
import gzip
import json
import os
import ssl
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

# Fork: https://github.com/robertgoley/searchdump-rs
# The image defaults to uid 65532. Dump directories belong to the invoking
# user, so the container runs as 0:0. Under rootless podman that uid is the
# invoking user; under rootful podman it matches the old Node image.
IMAGE = "sjc.vultrcr.com/eti1/searchdump-rs:0.1.0"
CONTAINER_USER = "0:0"
LIFECYCLE_MODES = ("drop", "convert")
# Data-file compression. gzip stays the default so existing backups restore
# unchanged. zstd1 and zstd3 are searchdump --compressionType zstd at
# levels 1 and 3; both write INDEX.data.json.zst.
COMPRESSIONS = {
    "gzip": ("gz", ("--fsCompress",)),
    "zstd1": ("zst", ("--compressionType=zstd", "--compressionLevel=1")),
    "zstd3": ("zst", ("--compressionType=zstd", "--compressionLevel=3")),
}
DATA_FLAGS = (
    "--limit=5000",
    "--scrollTime=10m",
    # Accepted for compatibility. searchdump always uses POST.
    "--scroll-with-post",
    "--retryAttempts=5",
    "--retryDelay=2000",
    "--timeout=300000",
    # The Rust default refuses a response body over 256 MiB. A page of
    # --limit=5000 log documents can exceed that. 0 leaves reads uncapped,
    # as the Node tool did.
    "--maxResponseMb=0",
)


class ClusterError(Exception):
    def __init__(self, message: str, exit_code: int = 2, http_status: int | None = None):
        super().__init__(message)
        self.exit_code = exit_code
        self.http_status = http_status


def die(message: str, code: int = 1) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(code)


def usage_text() -> str:
    name = Path(sys.argv[0]).name
    return f"""Usage: {name} backup --url URL --dir DIR --index INDEX [--compression gzip|zstd1|zstd3] [--username USER --password PASS]
       {name} restore --url URL --dir DIR --index INDEX [--compression gzip|zstd1|zstd3] [--lifecycle drop|convert] [--username USER --password PASS]

backup writes settings, mapping, and data for one index, in that order.
restore loads those files onto the named index, in that order.

--url is the cluster base URL (https://host:9200), without the index name.
--compression selects data-file compression. The default, gzip, writes
INDEX.data.json.gz. zstd1 and zstd3 write INDEX.data.json.zst at zstd
level 1 or 3. restore must name the compression used for that data file.
--lifecycle applies to restore and is passed through to searchdump.
The default, drop, passes
--excludeSettings=index.lifecycle,index.routing.allocation.include._tier_preference.
convert passes --convertIlmToIsm, which translates an ILM policy document
(--type policy) into an ISM policy. It does not rewrite index.lifecycle
inside a settings file.
Omit --username and --password when the cluster does not require auth.
If one of those two is set, the other is required.
backup creates the output directory if it does not already exist.
restore requires that directory and refuses to replace an index that already exists.
"""


def load_elasticdump_json(raw_text: str):
    # elasticdump writes settings as a JSON string of JSON.
    doc = json.loads(raw_text)
    if isinstance(doc, str):
        doc = json.loads(doc)
    return doc


class Cluster:
    def __init__(self, url: str, username: str, password: str, index: str):
        self.url = url
        self.username = username
        self.password = password
        self.index = index
        if username:
            token = f"{username}:{password}".encode()
            self._auth = "Basic " + base64.b64encode(token).decode()
        else:
            self._auth = None
        self._ssl = ssl._create_unverified_context()

    def index_path(self) -> str:
        return "/" + urllib.parse.quote(self.index, safe="")

    def request(self, method: str, path: str, body: bytes | None = None, timeout: int = 120):
        headers = {"Accept": "application/json"}
        if body is not None:
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(
            self.url + path,
            data=body,
            method=method,
            headers=headers,
        )
        if self._auth:
            req.add_header("Authorization", self._auth)
        try:
            with urllib.request.urlopen(req, context=self._ssl, timeout=timeout) as resp:
                raw = resp.read()
                if not raw:
                    return {}
                return json.loads(raw.decode())
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode(errors="replace")[:2000]
            raise ClusterError(
                f"{method} {path} -> HTTP {exc.code}: {detail}",
                exit_code=2,
                http_status=exc.code,
            ) from exc
        except urllib.error.URLError as exc:
            raise ClusterError(f"{method} {path} -> {exc.reason}", exit_code=2) from exc

    def exists(self) -> bool:
        # GET so a 401 keeps the body. HEAD on this cluster returns
        # content-length 29 and an empty body, which hid "Authentication finally failed".
        try:
            self.request("GET", self.index_path() + "/_settings", timeout=30)
        except ClusterError as exc:
            if exc.http_status == 404:
                return False
            if exc.http_status == 401:
                who = self.username if self.username else "no username was sent"
                die(f"{exc} (basic auth rejected for {who})", exc.exit_code)
            die(str(exc), exc.exit_code)
        return True

    def put_mapping(self, path: Path) -> None:
        doc = load_elasticdump_json(path.read_text(encoding="utf-8"))
        if not isinstance(doc, dict) or not doc:
            die("mapping dump is empty")
        _name, body = next(iter(doc.items()))
        mappings = body.get("mappings", body) if isinstance(body, dict) else body
        try:
            self.request(
                "PUT",
                self.index_path() + "/_mapping",
                json.dumps(mappings).encode(),
                timeout=180,
            )
        except ClusterError as exc:
            die(str(exc), exc.exit_code)

    def count(self) -> int:
        try:
            self.request("POST", self.index_path() + "/_refresh", timeout=180)
            doc = self.request("GET", self.index_path() + "/_count", timeout=180)
        except ClusterError as exc:
            die(str(exc), exc.exit_code)
        return int(doc["count"])


def lifecycle_flags(mode: str) -> tuple[str, ...]:
    # searchdump drops these on a settings restore. A file does not name
    # the source engine, so the tool keeps them unless --excludeSettings says so.
    if mode == "drop":
        return (
            "--excludeSettings=index.lifecycle,index.routing.allocation.include._tier_preference",
        )
    return ("--convertIlmToIsm",)


def data_file(directory: Path, index: str, compression: str) -> Path:
    suffix, _flags = COMPRESSIONS[compression]
    return directory / f"{index}.data.json.{suffix}"


def compression_flags(compression: str) -> tuple[str, ...]:
    return COMPRESSIONS[compression][1] + DATA_FLAGS


def present_data_files(directory: Path, index: str) -> list[Path]:
    found: list[Path] = []
    seen: set[str] = set()
    for suffix, _flags in COMPRESSIONS.values():
        if suffix in seen:
            continue
        seen.add(suffix)
        path = directory / f"{index}.data.json.{suffix}"
        if path.is_file() and path.stat().st_size > 0:
            found.append(path)
    return found


def require_data_file(directory: Path, index: str, compression: str) -> Path:
    path = data_file(directory, index, compression)
    if path.is_file() and path.stat().st_size > 0:
        return path
    others = [item for item in present_data_files(directory, index) if item != path]
    if others:
        names = ", ".join(item.name for item in others)
        if path.suffix == ".gz":
            hint = "pass --compression zstd1 or --compression zstd3"
        else:
            hint = "pass --compression gzip"
        die(f"missing data backup: {path} ({names} is present; {hint})")
    die(f"missing data backup: {path}")


def _stream_newlines(handle) -> int:
    count = 0
    while True:
        chunk = handle.read(1024 * 1024)
        if not chunk:
            return count
        count += chunk.count(b"\n")


def compressed_line_count(path: Path, compression: str) -> int:
    if compression == "gzip":
        with gzip.open(path, "rb") as handle:
            return _stream_newlines(handle)
    try:
        proc = subprocess.Popen(
            ["zstd", "-dc", "--stdout", "--quiet", "--", str(path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except FileNotFoundError:
        die("zstd: command not found", 127)
    if proc.stdout is None:
        die("zstd: no stdout")
    count = _stream_newlines(proc.stdout)
    err = proc.stderr.read() if proc.stderr is not None else b""
    code = proc.wait()
    if code != 0:
        detail = err.decode(errors="replace").strip()
        die(f"zstd could not read {path}: {detail or f'exit {code}'}")
    return count


def auth_env(username: str, password: str, *, output: bool) -> list[str]:
    lines = ["NODE_TLS_REJECT_UNAUTHORIZED=0"]
    if not username:
        return lines
    lines.append(f"SEARCHDUMP_INPUT_USERNAME={username}")
    lines.append(f"SEARCHDUMP_INPUT_PASSWORD={password}")
    if output:
        lines.append(f"SEARCHDUMP_OUTPUT_USERNAME={username}")
        lines.append(f"SEARCHDUMP_OUTPUT_PASSWORD={password}")
    return lines


def make_env_file(lines: list[str]) -> Path:
    fd, name = tempfile.mkstemp(prefix="oslogs-")
    os.close(fd)
    path = Path(name)
    try:
        os.chmod(path, 0o600)
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    except Exception:
        path.unlink(missing_ok=True)
        raise
    return path


def run_dump(
    env_file: Path,
    mounts: list[tuple[Path, str]],
    source: str,
    output: str,
    kind: str,
    extra: tuple[str, ...] = (),
) -> int:
    cmd = [
        "podman",
        "run",
        "--rm",
        "--network=host",
        "--user",
        CONTAINER_USER,
        "--env-file",
        str(env_file),
    ]
    for host, container in mounts:
        cmd.extend(["-v", f"{host}:{container}:Z"])
    cmd.extend(
        [
            IMAGE,
            f"--input={source}",
            f"--output={output}",
            f"--type={kind}",
            "--overwrite",
            *extra,
        ]
    )
    try:
        result = subprocess.run(cmd)
    except FileNotFoundError:
        die("podman: command not found", 127)
    return result.returncode


def require_success(code: int) -> None:
    if code != 0:
        raise SystemExit(code)


def backup_index(
    url: str,
    username: str,
    password: str,
    directory: str,
    index: str,
    compression: str,
) -> None:
    dest = Path(directory)
    dest.mkdir(parents=True, exist_ok=True)
    dest = Path(os.path.abspath(dest))
    source = f"{url}/{urllib.parse.quote(index, safe='')}"
    env_file = None
    try:
        env_file = make_env_file(auth_env(username, password, output=False))
        mounts = [(dest, "/dump")]
        require_success(
            run_dump(env_file, mounts, source, f"/dump/{index}.settings.json", "settings")
        )
        require_success(
            run_dump(env_file, mounts, source, f"/dump/{index}.mapping.json", "mapping")
        )
        data_path = data_file(dest, index, compression)
        require_success(
            run_dump(
                env_file,
                mounts,
                source,
                f"/dump/{data_path.name}",
                "data",
                compression_flags(compression),
            )
        )
    finally:
        if env_file is not None:
            env_file.unlink(missing_ok=True)

    for kind in ("settings", "mapping"):
        path = dest / f"{index}.{kind}.json"
        if not path.is_file() or path.stat().st_size == 0:
            die(f"{kind} export for {index} is empty")

    data_path = data_file(dest, index, compression)
    records = compressed_line_count(data_path, compression)
    if records < 1:
        die(f"data export for {index} is empty")

    print(f"exported {index} to {dest}")
    print(f"  settings {dest}/{index}.settings.json")
    print(f"  mapping  {dest}/{index}.mapping.json")
    print(f"  data     {data_path} ({records} records)")


def restore_index(
    url: str,
    username: str,
    password: str,
    directory: str,
    index: str,
    compression: str,
    lifecycle: str,
) -> None:
    dest = Path(directory)
    if not dest.is_dir():
        die(f"backup directory does not exist: {directory}")
    dest = Path(os.path.abspath(dest))
    settings = dest / f"{index}.settings.json"
    mapping = dest / f"{index}.mapping.json"
    data = require_data_file(dest, index, compression)
    if not settings.is_file() or settings.stat().st_size == 0:
        die(f"missing settings backup: {settings}")
    if not mapping.is_file() or mapping.stat().st_size == 0:
        die(f"missing mapping backup: {mapping}")

    client = Cluster(url, username, password, index)
    if client.exists():
        die(f"index {index} already exists on {url}")

    target = f"{url}/{urllib.parse.quote(index, safe='')}"
    env_file = None
    try:
        env_file = make_env_file(auth_env(username, password, output=True))
        mounts = [(dest, "/dump")]
        settings_flags = lifecycle_flags(lifecycle)

        print(f"restore settings for {index}")
        print("searchdump " + " ".join(settings_flags))
        require_success(
            run_dump(
                env_file,
                mounts,
                f"/dump/{index}.settings.json",
                target,
                "settings",
                settings_flags,
            )
        )

        print(f"restore mapping for {index}")
        mapping_code = run_dump(
            env_file, mounts, f"/dump/{index}.mapping.json", target, "mapping"
        )
        if mapping_code != 0:
            print("mapping via searchdump failed, applying directly", file=sys.stderr)
            client.put_mapping(mapping)

        print(f"restore data for {index}")
        require_success(
            run_dump(
                env_file,
                mounts,
                f"/dump/{data.name}",
                target,
                "data",
                compression_flags(compression),
            )
        )
    finally:
        if env_file is not None:
            env_file.unlink(missing_ok=True)

    records = compressed_line_count(data, compression)
    restored = client.count()
    if restored != records:
        die(f"restored {index} docs={restored}, backup file has {records} records")
    print(f"restored {index} docs={restored}")


def normalize_credential(value: str) -> str:
    # searchdump drops a trailing CR/LF. A password that keeps one
    # never matches and the cluster returns 401.
    return value.rstrip("\r\n")


def take_value(flag: str, args: list[str], index: int) -> str:
    if index + 1 >= len(args) or args[index + 1] == "" or args[index + 1].startswith("--"):
        die(f"missing value for {flag}")
    return args[index + 1]


def parse_args(argv: list[str]) -> tuple[str, str, str, str, str, str, str, str]:
    if not argv:
        print(usage_text(), file=sys.stderr, end="")
        raise SystemExit(1)
    if argv[0] in ("-h", "--help"):
        print(usage_text(), end="")
        raise SystemExit(0)
    command = argv[0]
    if command not in ("backup", "restore"):
        die(f"unknown command: {command}")

    url = ""
    username = ""
    password = ""
    directory = ""
    index = ""
    compression = "gzip"
    lifecycle = "drop"
    args = argv[1:]
    cursor = 0
    while cursor < len(args):
        flag = args[cursor]
        if flag in ("-h", "--help"):
            print(usage_text(), end="")
            raise SystemExit(0)
        if flag == "--url":
            url = take_value(flag, args, cursor)
        elif flag == "--username":
            username = take_value(flag, args, cursor)
        elif flag == "--password":
            password = take_value(flag, args, cursor)
        elif flag == "--dir":
            directory = take_value(flag, args, cursor)
        elif flag == "--index":
            index = take_value(flag, args, cursor)
        elif flag == "--compression":
            compression = take_value(flag, args, cursor)
        elif flag == "--lifecycle":
            lifecycle = take_value(flag, args, cursor)
        else:
            die(f"unknown argument: {flag}")
        cursor += 2

    username = normalize_credential(username)
    password = normalize_credential(password)
    if not url:
        die("missing --url")
    if username and not password:
        die("missing --password")
    if password and not username:
        die("missing --username")
    if not directory:
        die("missing --dir")
    if not index:
        die("missing --index")
    if not url.startswith(("http://", "https://")):
        die("--url must start with http:// or https://")
    if "/" in index or index in (".", ".."):
        die("--index must be a single index name")
    if compression not in COMPRESSIONS:
        die("--compression must be gzip, zstd1, or zstd3")
    if lifecycle not in LIFECYCLE_MODES:
        die("--lifecycle must be drop or convert")
    if command == "backup" and lifecycle != "drop":
        die("--lifecycle applies to restore")
    if url.endswith("/"):
        url = url[:-1]
    return command, url, username, password, directory, index, compression, lifecycle


def main(argv: list[str] | None = None) -> None:
    if argv is None:
        argv = sys.argv[1:]
    command, url, username, password, directory, index, compression, lifecycle = parse_args(argv)
    if command == "backup":
        backup_index(url, username, password, directory, index, compression)
        return
    restore_index(url, username, password, directory, index, compression, lifecycle)


if __name__ == "__main__":
    main()
