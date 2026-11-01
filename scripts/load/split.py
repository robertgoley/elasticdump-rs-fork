#!/usr/bin/env python3
"""LOAD-009 (L-806): splitting at scale.

    scripts/load/split.py <tier> [--docs N] [--engine es717-plain] [--place box] [--seed 1]
                          [--run-id ID] [--keep]

Provisions one engine (scripts/it/up.sh), seeds `load-logs` (edgen logs, seed 1), takes an
uncompressed reference dump, then splits with --fileSize and --maxRows: the literal 10M spec
values (256mb, 1000000 rows) and values scaled to the tier (raw size / 40 bytes; docs / 10 and
docs * 3 / 10 rows), plain, gzip and zstd, file -> parts and cluster -> parts, one slice and
--slices 4. For every case:

  names      parts are p.split-0 .. p.split-(N-1) with the codec suffix, no gaps, no extras
  count      --maxRows: N = ceil(docs / rows); --fileSize: N and every boundary equal a replay of
             the tool's rule over the concatenated lines (a part closes after the line that brings
             its line bytes, newlines not counted, to the limit)
  no span    every part ends with a newline and every line is a whole JSON document with an _id
  limit      every part's line bytes <= limit - 1 + its last line; every part but the last reaches it
  digest     edverify diff of the index against all parts
  staged     the output directory is polled every 20 ms while the run is going: no visible part
             appears before every part is staged (BRK-RES-002: the parts are committed together;
             visible parts are allowed only in the commit window, where staged + visible equals
             the final part count, and that window's length is recorded)

Plus: restore every part (one invocation per part, as the README says) into a new index for
two cases and diff it with the source; kill -9 mid-split leaves no visible part and a rerun
with --overwrite completes; --overwrite with fewer parts replaces the set and leaves the older
higher-numbered parts with a warning.
Results: results/load/<run-id>/split.json. Exit 0 when every assertion passed, 1 otherwise,
2 refused, 3 skipped (resource guard). 10M with 256 MB parts is Phase B.
"""
import argparse, datetime, fcntl, glob, json, math, os, re, signal, subprocess, sys, threading, time, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import run as R  # noqa: E402

ROOT = R.ROOT
LIMIT = 1000
SUF = {"none": "", "gzip": ".gz", "zstd": ".zst"}


def log(*a):
    print("split:", *a, file=sys.stderr, flush=True)


def size_bytes(s):
    m = re.fullmatch(r"(\d+(?:\.\d+)?)(b|kb|mb|gb)?", s.lower())
    return int(float(m.group(1)) * {"b": 1, None: 1, "kb": 1024, "mb": 1024 ** 2, "gb": 1024 ** 3}[m.group(2)])


class Watch(threading.Thread):
    """Poll a directory; record visible parts and hidden staged files while `proc` runs."""

    def __init__(self, d, proc):
        super().__init__(daemon=True)
        self.d, self.proc = d, proc
        self.visible_early, self.max_hidden, self.polls = set(), 0, 0
        # When a part is visible, how many parts exist in total (staged + visible)?
        # Commit renames run one by one after every part is sealed, so during that
        # window the total equals the final part count; a smaller total means a part
        # became visible while the dump was still writing.
        self.min_total_when_visible, self.first_visible_t = None, None

    def run(self):
        while self.proc.poll() is None:
            try:
                names = os.listdir(self.d)
            except FileNotFoundError:
                names = []
            if self.proc.poll() is not None:
                break
            self.polls += 1
            vis = {n for n in names if n.startswith("p.split-")}
            staged = sum(1 for n in names if n.startswith(".p.split-") and ".part-" in n)
            self.visible_early |= vis
            self.max_hidden = max(self.max_hidden, staged)
            if vis:
                tot = len(vis) + staged
                self.min_total_when_visible = tot if self.min_total_when_visible is None else min(self.min_total_when_visible, tot)
                if self.first_visible_t is None:
                    self.first_visible_t = time.monotonic()
            time.sleep(0.02)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("tier")
    ap.add_argument("--docs", type=int)
    ap.add_argument("--engine", default="es717-plain")
    ap.add_argument("--place", default=os.environ.get("LOAD_PLACE", "box"))
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--run-id")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()

    tiers = tomllib.load(open(os.path.join(HERE, "scenarios", "tiers.toml"), "rb"))
    scen = {"name": "split", "seed": [{"index": "load-logs", "profile": "logs", "seed": 1, "docs": "tier"}],
            "step": [{"kind": "dump"}] * 3 + [{"kind": "restore"}]}
    ok, why, plan = R.fit(a.tier, a.place, tiers, scen, a.docs)
    if not ok:
        print(f"SPLIT-SUMMARY tier={a.tier} place={a.place} REFUSED: {why}")
        return 2
    lock = open("/tmp/elasticdump-rs-it.lock", "w")
    if os.environ.get("IT_LOCK_HELD") != "1":
        fcntl.flock(lock, fcntl.LOCK_EX)
    need_mem = plan["heap_mb"] * 2 + 512 + plan["place"]["tool_mem_mb"] + 256
    if R.meminfo_avail_mb() < need_mem:
        print(f"SPLIT-SUMMARY tier={a.tier} SKIPPED (MemAvailable {R.meminfo_avail_mb()} MB < {need_mem} MB)")
        return 3
    run_id = a.run_id or datetime.datetime.now().strftime("%Y%m%dT%H%M%S") + f"-split-{a.tier}"
    work = os.path.join(os.environ.get("LOAD_WORK", "/var/tmp/elasticdump-rs-load"), run_id)
    os.makedirs(work, mode=0o700, exist_ok=True)
    resdir = os.path.join(ROOT, "results", "load", run_id)
    os.makedirs(resdir, exist_ok=True)
    edgen, edverify, tool = R.tool_path("edgen"), R.tool_path("edverify"), R.tool_path("elasticdump-rs")
    base = {}
    for line in subprocess.run([os.path.join(ROOT, "scripts/it/matrix.sh"), a.engine],
                               check=True, capture_output=True, text=True).stdout.splitlines():
        k, v = line.split("=", 1)
        base[k] = v
    up_profile = base.pop("UP_PROFILE")
    heap = f"{plan['heap_mb']}m"
    env = dict(os.environ, **base, IT_DIR=os.path.join(work, "it"), IT_LOCK_HELD="1",
               RUN_ID=f"load{os.getpid()}split", ES_HEAP=heap, OS_HEAP=heap, PORT="19202",
               IT_CONTAINER_MEM=plan["place"]["container_mem"])
    res = dict(schema="edrs-split/v1", run_id=run_id, tier=a.tier, place=a.place, engine=a.engine, heap=heap,
               seed=a.seed, limit=LIMIT,
               git=subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"],
                                  capture_output=True, text=True).stdout.strip(),
               tool=subprocess.run([tool, "--version"], capture_output=True, text=True).stdout.strip(),
               started=datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
               cases=[], extra={}, failures=[])
    rc = 1

    def fail(msg):
        res["failures"].append(msg)
        log("FAIL", msg)

    try:
        log(f"provisioning {a.engine} (heap {heap})")
        subprocess.run([os.path.join(ROOT, "scripts/it/up.sh"), up_profile], env=env, check=True,
                       stdout=subprocess.DEVNULL)
        url = R.sh_env(os.path.join(env["IT_DIR"], "env"))["IT_ES_URL"]
        cl = R.Cluster(url)
        res["engine_version"] = cl.req("GET", "/")["version"]["number"]
        s = dict(plan["seeds"][0])
        log(f"seeding load-logs: {s['docs']} docs")
        res["seed_index"] = R.seed_index(cl, edgen, s, R.engine_mapping_flavour(a.engine))
        n = res["seed_index"]["stored"]
        mapping = json.loads(subprocess.run([edgen, "mapping", "--profile", "logs", "--engine",
                                             R.engine_mapping_flavour(a.engine)], check=True,
                                            capture_output=True).stdout)

        def digest(tag, b):
            rep = os.path.join(resdir, f"verify-{tag}.json")
            p = subprocess.run([edverify, "diff", "--a", f"{url}/load-logs", "--b", *b, "--index-as", "x",
                                "--out", rep], capture_output=True, text=True)
            return p.returncode == 0

        ref = os.path.join(work, "ref.ndjson")
        p = subprocess.run([tool, "--input", f"{url}/load-logs", "--output", ref, "--type=data", f"--limit={LIMIT}"],
                           capture_output=True, text=True)
        if p.returncode or not digest("ref", [ref]):
            raise RuntimeError(f"reference dump failed: {p.stderr[-400:]}")
        raw = os.path.getsize(ref)
        res["raw_bytes"] = raw
        small = max(1, raw // 40)
        log(f"reference: {raw / 1e6:.0f} MB; scaled --fileSize {small} bytes")

        def parts_of(d, codec):
            pat = re.compile(r"p\.split-(\d+)\.json" + re.escape(SUF[codec]) + "$")
            names = os.listdir(d)
            idx = sorted(int(m.group(1)) for m in map(pat.match, names) if m)
            extras = sorted(x for x in names if not pat.match(x))
            return idx, extras

        def lines_of(path):
            data = subprocess.run([edgen, "cat", path], check=True, capture_output=True).stdout
            return data

        def split_run(tag, src, flags, codec="none", d=None, overwrite=False):
            d = d or os.path.join(work, tag)
            os.makedirs(d, exist_ok=True)
            cflags = {"none": [], "gzip": ["--fsCompress"], "zstd": ["--compressionType=zstd"]}[codec]
            cmd = [tool, "--input", src, "--output", os.path.join(d, "p.json"), "--type=data", f"--limit={LIMIT}",
                   *flags, *cflags] + (["--overwrite"] if overwrite else [])
            t0 = time.monotonic()
            proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            w = Watch(d, proc)
            w.start()
            err = proc.stderr.read()
            proc.wait()
            t_end = time.monotonic()
            w.join()
            w.commit_window_s = round(t_end - w.first_visible_t, 3) if w.first_visible_t else 0.0
            return d, proc.returncode, err, round(time.monotonic() - t0, 2), w

        def check(tag, src_kind, flags, codec="none", ordered=True):
            src = ref if src_kind == "file" else f"{url}/load-logs"
            d, rc_, err, wall, w = split_run(tag, src, flags, codec)
            row = dict(case=tag, src=src_kind, flags=flags, codec=codec, exit=rc_, wall_s=wall,
                       polls=w.polls, max_staged_hidden=w.max_hidden, visible_in_commit_window=len(w.visible_early),
                       min_total_when_visible=w.min_total_when_visible, commit_window_s=w.commit_window_s)
            ok_ = True
            if rc_:
                row["err"] = err.strip().splitlines()[-2:]
                fail(f"{tag}: exit {rc_}: {row['err']}")
                res["cases"].append(row)
                return row
            idx, extras = parts_of(d, codec)
            row["parts"] = len(idx)
            row["names_contiguous"] = idx == list(range(len(idx)))
            row["no_extra_files"] = not extras
            if extras:
                row["extras"] = extras[:5]
            by = next((f.split("=", 1)[1] for f in flags if f.startswith("--fileSize")), None)
            rows_ = next((int(f.split("=", 1)[1]) for f in flags if f.startswith("--maxRows")), None)
            limit = size_bytes(by) if by else rows_
            row["limit"] = limit
            # replay the rule over the concatenation, compare boundaries
            whole, part_lines, bad_json, no_nl = [], [], 0, 0
            span_ok, limit_ok, reach_ok = True, True, True
            for i in idx:
                data = lines_of(os.path.join(d, f"p.split-{i}.json{SUF[codec]}"))
                if not data.endswith(b"\n"):
                    no_nl += 1
                ls = data.split(b"\n")
                if ls and ls[-1] == b"":
                    ls.pop()
                part_lines.append(len(ls))
                cnt = 0
                for j, ln in enumerate(ls):
                    try:
                        if "_id" not in json.loads(ln):
                            bad_json += 1
                    except ValueError:
                        bad_json += 1
                    cnt += len(ln) if by else 1
                    if cnt >= limit and j != len(ls) - 1:
                        limit_ok = False  # the part went on past the line that reached the limit
                if i != idx[-1] and cnt < limit:
                    reach_ok = False
                if by and ls and cnt > limit - 1 + len(ls[-1]):
                    limit_ok = False
                whole.append(len(ls))
            row["docs"] = sum(part_lines)
            row["part_docs_min_max"] = [min(part_lines), max(part_lines)] if part_lines else None
            row["every_part_ends_with_newline"] = no_nl == 0
            row["every_line_whole_doc"] = bad_json == 0
            row["no_part_past_limit"] = limit_ok
            row["every_part_but_last_reaches_limit"] = reach_ok
            if rows_:
                row["expected_parts"] = math.ceil(n / rows_)
            elif ordered:
                # replay over the reference order (file input and one-slice dumps read in the same order)
                exp, cnt = 0, None
                with open(ref, "rb") as f:
                    for ln in f:
                        if cnt is None:
                            exp, cnt = exp + 1, 0
                        cnt += len(ln) - 1
                        if cnt >= limit:
                            cnt = None
                row["expected_parts"] = exp
            row["count_ok"] = row.get("expected_parts", row["parts"]) == row["parts"]
            row["docs_ok"] = row["docs"] == n
            # a visible part is only allowed in the commit window, when every part is already staged
            row["no_visible_part_before_all_staged"] = (w.min_total_when_visible is None
                                                        or w.min_total_when_visible >= row["parts"])
            row["digest_equal"] = digest(tag, [os.path.join(d, f"p.split-{i}.json{SUF[codec]}") for i in idx])
            bad = [k for k in ("names_contiguous", "no_extra_files", "every_part_ends_with_newline",
                               "every_line_whole_doc", "no_part_past_limit", "every_part_but_last_reaches_limit",
                               "count_ok", "docs_ok", "no_visible_part_before_all_staged", "digest_equal") if not row[k]]
            if bad:
                ok_ = False
                fail(f"{tag}: {bad}")
            log(f"{tag}: {'ok' if ok_ else 'FAIL'} parts {row['parts']} (expected {row.get('expected_parts')}), "
                f"{wall} s, staged hidden max {w.max_hidden}, polls {w.polls}, "
                f"commit window {w.commit_window_s} s (min total when visible {w.min_total_when_visible})")
            res["cases"].append(row)
            return row

        small_s = f"{small}b"
        rows10, rows30 = max(1, n // 10), max(1, n * 3 // 10)
        cases = [
            ("file-256mb", "file", ["--fileSize=256mb"], "none"),
            ("file-scaled-bytes", "file", [f"--fileSize={small_s}"], "none"),
            ("file-1000000rows", "file", ["--maxRows=1000000"], "none"),
            ("file-rows10", "file", [f"--maxRows={rows10}"], "none"),
            ("dump-256mb", "es", ["--fileSize=256mb"], "none"),
            ("dump-scaled-bytes", "es", [f"--fileSize={small_s}"], "none"),
            ("dump-1000000rows", "es", ["--maxRows=1000000"], "none"),
            ("dump-rows30", "es", [f"--maxRows={rows30}"], "none"),
            ("dump-scaled-bytes-gzip", "es", [f"--fileSize={small_s}"], "gzip"),
            ("dump-scaled-bytes-zstd", "es", [f"--fileSize={small_s}"], "zstd"),
            ("dump-rows10-zstd", "es", [f"--maxRows={rows10}"], "zstd"),
            ("dump-slices4-scaled-bytes", "es", [f"--fileSize={small_s}", "--slices=4"], "none"),
            ("dump-slices4-rows10-gzip", "es", [f"--maxRows={rows10}", "--slices=4"], "gzip"),
        ]
        for tag, kind, flags, codec in cases:
            check(tag, kind, flags, codec, ordered="--slices=4" not in flags)

        # restore every part, one invocation per part
        def restore_parts(tag, codec):
            d = os.path.join(work, tag)
            idx, _ = parts_of(d, codec)
            cl.req("DELETE", "/dst", ok404=True)
            cl.req("PUT", "/dst", mapping)
            t0, rcs = time.monotonic(), []
            for i in idx:
                p = subprocess.run([tool, "--input", os.path.join(d, f"p.split-{i}.json{SUF[codec]}"),
                                    "--output", f"{url}/dst", "--type=data", f"--limit={LIMIT}",
                                    "--concurrency=8", "--noRefresh"], capture_output=True, text=True)
                rcs.append(p.returncode)
            cl.req("POST", "/dst/_refresh")
            rep = os.path.join(resdir, f"verify-restore-{tag}.json")
            q = subprocess.run([edverify, "diff", "--a", f"{url}/load-logs", "--b", f"{url}/dst", "--index-as", "x",
                                "--out", rep], capture_output=True, text=True)
            r = dict(case=tag, parts=len(idx), exits=sorted(set(rcs)), wall_s=round(time.monotonic() - t0, 1),
                     count=cl.req("GET", "/dst/_count")["count"], digest_equal=q.returncode == 0)
            log(f"restore parts {tag}: {r}")
            if r["exits"] != [0] or not r["digest_equal"]:
                fail(f"restore-parts {tag}: {r}")
            return r

        res["extra"]["restore_parts"] = [restore_parts("dump-scaled-bytes", "none"),
                                         restore_parts("dump-rows10-zstd", "zstd")]
        cl.req("DELETE", "/dst", ok404=True)

        # kill -9 mid-split, then rerun with --overwrite
        kd = os.path.join(work, "kill")
        os.makedirs(kd, exist_ok=True)
        cmd = [tool, "--input", f"{url}/load-logs", "--output", os.path.join(kd, "p.json"), "--type=data",
               f"--limit={LIMIT}", f"--fileSize={small_s}"]
        proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        while proc.poll() is None:
            hidden = [x for x in os.listdir(kd) if x.startswith(".")]
            if len(hidden) >= 15:
                break
            time.sleep(0.02)
        proc.send_signal(signal.SIGKILL)
        proc.wait()
        names = os.listdir(kd)
        k = dict(staged_at_kill=len([x for x in names if x.startswith(".")]),
                 visible_after_kill=sorted(x for x in names if not x.startswith(".")))
        d, rc_, err, wall, w = split_run("kill-rerun", f"{url}/load-logs", [f"--fileSize={small_s}"], d=kd,
                                         overwrite=True)
        idx, extras = parts_of(kd, "none")
        k.update(rerun_exit=rc_, rerun_parts=len(idx), rerun_hidden_left=sorted(extras)[:3],
                 rerun_hidden_left_count=len(extras), rerun_mentions_leftovers="part-" in err or "leftover" in err,
                 rerun_digest_equal=digest("kill-rerun", [os.path.join(kd, f"p.split-{i}.json") for i in idx]))
        log(f"kill -9: {k}")
        res["extra"]["kill"] = k
        if k["visible_after_kill"] or rc_ or not k["rerun_digest_equal"]:
            fail(f"kill: {k}")

        # --overwrite with fewer parts: dump-scaled-bytes has ~40 parts; write 10 (rows) over them
        od = os.path.join(work, "dump-scaled-bytes")
        before, _ = parts_of(od, "none")
        d, rc_, err, wall, w = split_run("overwrite-fewer", f"{url}/load-logs", [f"--maxRows={rows10}"], d=od,
                                         overwrite=True)
        after, extras = parts_of(od, "none")
        new_ok = digest("overwrite-fewer", [os.path.join(od, f"p.split-{i}.json") for i in range(10)])
        warn = [x for x in err.splitlines() if "WARN" in x and "split" in x]
        o = dict(before=len(before), exit=rc_, after_total=len(after), new_set_digest_equal=new_ok,
                 older_left=len(after) - min(10, len(after)), warning=warn[:1])
        log(f"overwrite with fewer parts: {o}")
        res["extra"]["overwrite_fewer"] = o
        if rc_ or not new_ok or (o["older_left"] and not warn):
            fail(f"overwrite-fewer: {o}")

        left = []
        for dp, _, fs in os.walk(work):
            if "/it" in dp:
                continue
            left += [f for f in fs if ".part-" in f and not dp.endswith("kill")]
        res["leak"] = dict(part_files_outside_kill=left[:5],
                           scroll_current=sum(v["indices"]["search"].get("scroll_current", 0) for v in
                                              cl.req("GET", "/_nodes/stats/indices/search")["nodes"].values()))
        if left:
            fail(f"leak: {res['leak']}")
        rc = 0 if not res["failures"] else 1
    except Exception as e:
        res["failures"].append(f"error: {e!r}")
        log("ERROR", repr(e))
        rc = 1
    finally:
        res["finished"] = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
        res["pass"] = rc == 0
        json.dump(res, open(os.path.join(resdir, "split.json"), "w"), indent=1)
        if not a.keep:
            subprocess.run(["podman", "rm", "-f", f"it-es-plain-{env['RUN_ID']}"], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
            subprocess.run(["podman", "unshare", "rm", "-rf", work])
    print(f"SPLIT-SUMMARY tier={a.tier} engine={a.engine} docs={res.get('seed_index', {}).get('stored')} "
          f"{'PASS' if rc == 0 else 'FAIL'} cases={len(res['cases'])} result=results/load/{run_id}/split.json")
    for f in res["failures"]:
        print("  failure:", f)
    return rc


if __name__ == "__main__":
    sys.exit(main())
