# Examples

Worked dump and restore commands for [searchdump-rs](../README.md). They use only flags that
exist on current `main` (the 1.0.0 line). They do not use 1.1-only flags such as
`--refreshInterval` or `--sliceCap`.

These are starting points, not a production runbook. Adjust URLs, index names, resource
limits and page sizes for your cluster, and test outside production first.

**Disclaimer:** searchdump-rs is provided as is, without warranty of any kind. Read the
[Disclaimer](../README.md#disclaimer) and the [MIT license](../LICENSE) before you rely on
a backup. Verify every dump by restoring it and comparing document counts (and spot-checking
documents, mappings and settings) against the source.

Credentials belong in `SEARCHDUMP_INPUT_USERNAME` / `SEARCHDUMP_INPUT_PASSWORD`,
`SEARCHDUMP_OUTPUT_*`, `--httpAuthFile`, or `--inputStorageConfig` / `--outputStorageConfig`
(or the `SEARCHDUMP_*_STORAGE_CONFIG` environment variables). Never put a password or access
key in a URL. The templates in this directory leave secret values blank; copy them, fill in a
private file (`chmod 600`), and do not commit the filled copy.

The default container image in the Kubernetes and Docker/Podman examples is the project's
dhi.io Debian image with a shell:

- published: `ghcr.io/robertgoley/searchdump-rs:1.0.0-dhi-debian`
- local `make image` / `scripts/build-image.sh`: `localhost/searchdump-rs:1.0.0` (and
  `:latest`)

The image entrypoint is `searchdump`; `searchdump-multi` is next to it. It runs as uid 65532,
and `/data` is the writable working directory. The shell-less `dhi-static` image is not used
here, because several examples run a short `sh` loop (settings, then mapping, then data).
`sftp://` does not work inside the images yet; use `s3://`, `b2://`, `fs://`, or the host
binaries for SFTP. See the [container image](../README.md#container-image) section of the
README.

## Layout

| Directory | What it is |
|---|---|
| [`bash/`](bash/) | Runnable scripts that call `searchdump` / `searchdump-multi` on the host (`PATH`, or `SEARCHDUMP_BIN` / `SEARCHDUMP_MULTI_BIN`) |
| [`docker-podman/`](docker-podman/) | The same cases, run with `podman` (default) or `docker` (`CONTAINER_ENGINE=docker`) |
| [`kubernetes/`](kubernetes/) | Job manifests, plus a shared PVC for the dump directory. Secrets are referenced, not filled in |

Each case has a dump and a restore (cluster-to-cluster copy is a single run). Shared templates:
[`http-auth.ini.example`](http-auth.ini.example), [`storage-config.ini.example`](storage-config.ini.example).

## Cases and tradeoffs

### Minimum resources

Small pages, one writer at a time, low memory and CPU. Use this on a laptop, a small CI job, or
a Bitnami / constrained Elasticsearch or OpenSearch node.

- `searchdump` already defaults to `--limit=100` and `--concurrency=1`. The examples also set
  `--workers=1`, a modest `--maxResponseMb`, and (on restore) `--concurrency=1`.
- **`searchdump-multi` default data flags are too heavy for a small heap.** On current `main`,
  every data child gets `--limit=5000`, `--maxResponseMb=0` (no response-size cap) and
  `--scrollTime=10m`, from `DATA_FLAGS` in `src/bin/searchdump-multi.rs`. That can return HTTP
  429 (`circuit_breaking_exception` or a rejected execution) or OOM the node during a **dump**,
  even with one thread and one shard, which is what we saw against bitnamilegacy Elasticsearch.
  Until 1.1-10 ships, pass `-- --limit=500 --maxResponseMb=64` (a flag after `--` replaces the
  default of the same name) or `--noDefaultDataFlags` and set `--limit` yourself. Also use
  `--parallel=1`. A fix (lower default limit, keep a response cap on, retry with a smaller page
  on 429 / circuit breaking, clear scroll contexts on exit) is on the 1.1 list in
  [`FEATURES-TODO.md`](../FEATURES-TODO.md).

Gzip (or zstd level 1) is fine here; the point is the small page size, not the codec.

### Fastest dump (and a matching restore)

Higher `--limit`, more `--workers`, and, when the engine supports it, sliced PIT
(`--searchAfter --pit --slices=N`). Compression defaults to zstd level 1, which is much
faster to write than gzip-6 and still compact. `--esCompress` asks the cluster for HTTP
compression on responses.

Set `--slices` to the **primary shard count** of the index, not higher. On Elasticsearch 7.10
and earlier (and some OpenSearch builds) too many slices fall back to `_id` slicing and can
be slower than an unsliced scroll. There is no `--sliceCap` on `main`; you have to pick `N`
yourself.

On restore and on cluster-to-cluster copy, raise `--concurrency` (concurrent write batches)
and consider `--noRefresh` so the cluster is not asked to refresh after every bulk batch.
`--noRefresh` exists on `main`; do not invent `--refreshInterval`.

Faster also means heavier: more client CPU and memory, and more load on the cluster.

### Very large index

Split the data file with `--fileSize` (for example `1gb`) or `--maxRows`, and give the scroll
a longer `--scrollTime` (for example `30m`) so a slow page does not expire the context.
`searchdump-multi` records finished indices in `multielasticdump.json` and `--resume` skips
those whose files are still present. A single `searchdump` run has no `--resume`; if it fails
mid-index you usually start that index again. A failed split run may delete the parts it
wrote; an interrupted one can leave some, so check the dump directory before you retry
(`--overwrite` replaces existing parts).

Restoring a split dump with `searchdump` takes one invocation per part, in order, as in Node.
`searchdump-multi --direction=load` finds the parts and loads them in order itself.

### Secure TLS and auth

Use `--httpAuthFile` or the `SEARCHDUMP_*` environment variables, plus `--ca` / `--cert` /
`--key` (or the `--input-*` / `--output-*` forms) for a private CA and optional client
certificates. Do not use `--insecure` in these examples.

`searchdump-multi` has `--httpAuthFile` and `--insecure`, but no `--ca` / `--cert` / `--key`.
Its own cluster calls (listing indices, counts) use the process's default trust store, which
honours `SSL_CERT_FILE` / `SSL_CERT_DIR` via rustls-native-certs. For a private CA, point
`SSL_CERT_FILE` at a bundle that includes it, or install the CA in the system store. mTLS
for `searchdump-multi` is not fully wired on `main` (a `-- --cert= --key=` after `--` reaches
only data children, not the parent or settings/mapping runs). Use `searchdump` when you need
client certificates.

### Object storage

Dump to or restore from `s3://bucket/key` (MinIO and other S3-compatible endpoints work the
same). Put `endpoint`, `region`, `key` and `secret` in an INI file passed as
`--outputStorageConfig` / `--inputStorageConfig`, or in `SEARCHDUMP_S3_*` / `AWS_*`. Never
put keys in the URL; the tool refuses a URL that contains a secret. `--overwrite` is required
to replace an existing object. The same pattern works with `b2://`. See
[Object storage and SFTP](../README.md#object-storage-and-sftp-opendal).

### Cluster-to-cluster copy

One `searchdump` process per item type (`settings`, `mapping`, then `data`) from
`--input=https://src:9200/index` to `--output=https://dst:9200/index`. Use
`SEARCHDUMP_INPUT_*` and `SEARCHDUMP_OUTPUT_*` (or `--httpAuthFile` with
`--input-username` / `--output-username` when the users differ). This is not
`searchdump-multi --direction=copy`, which copies a dump **directory** between stores and
does not talk to a cluster.

### Multi-index (`searchdump-multi`)

`--match` selects indices, `--parallel` runs several at a time, `--compression` sets the data
codec (`gzip`, `zstd1`, `zstd3`, `none`). Load with `--direction=load`. Add `--resume` on a
retry, and `--skipExisting` on load if some target indices already exist. On a small-heap
source, still pass `-- --limit=500` as in [Minimum resources](#minimum-resources).

## How to run them

**Bash.** Install the [host binaries](../README.md#install-and-build) (or a release archive)
so `searchdump` and `searchdump-multi` are on `PATH`. From the repo root:

```bash
export ES_URL=https://localhost:9200 INDEX=logs
# export SEARCHDUMP_INPUT_USERNAME SEARCHDUMP_INPUT_PASSWORD   # or HTTP_AUTH_FILE=/path/http-auth.ini
examples/bash/minimum-resources-dump.sh
examples/bash/minimum-resources-restore.sh
```

Every bash and Docker/Podman script starts with `set -euo pipefail` and is configured through
the environment (see comments at the top of each `_common.sh`).

**Docker / Podman.** Default engine is `podman`; use `CONTAINER_ENGINE=docker` if you prefer
Docker. The default `--network=host` is convenient on Linux so `ES_URL=http://127.0.0.1:9200`
reaches a cluster on the host. The dump directory is mounted at `/data`. Uid 65532 must be
able to write it (the scripts create the directory and explain the permission options).

```bash
export ES_URL=http://127.0.0.1:9200 INDEX=logs
# SEARCHDUMP_IMAGE=localhost/searchdump-rs:1.0.0
examples/docker-podman/minimum-resources-dump.sh
```

**Kubernetes.** Create the PVC and the Secrets the Jobs refer to, then apply one Job at a
time. Details are in [`kubernetes/README.md`](kubernetes/README.md).

## Further ideas

Not given as full files, but useful once the starters fit:

- A filtered dump with `--searchBody=@query.json` (or `--searchWithTemplate`)
- Data streams: `searchdump-multi --matchType=datastream --types=data,component_template,index_template,policy`
- ILM to ISM on load: `searchdump-multi --lifecycle=convert` (see the README before using this on OpenSearch)
- Node-compatible layout: `searchdump-multi --nodeCompat`
- A Kubernetes `CronJob` around any of the dump Jobs, for a scheduled backup
- `searchdump-multi --direction=copy` to move an existing dump directory between `s3://`, `b2://`, `sftp://` (host binaries) or a local path
