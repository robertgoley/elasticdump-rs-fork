#!/usr/bin/env python3
"""TST-SUP-016 (L-705): release-candidate behaviour profile and diff.

A *profile* condenses one SUP-002 trace run (scripts/sup/trace.sh, observed.tsv) and one SUP-001
egress run (scripts/sup/egress.sh, dns.log + egress-summary.log) into a JSON file that does not
change between runs of the same code on the same kind of machine:
  trace.entries   sorted "kind<TAB>value" (exec/path/connect/bind/socket/process/syscall), with
                  run-specific parts normalised (staging-file pid/nanos, files in the system CA store,
                  the host's cgroup path, numeric suffixes of node names)
  trace.cases     case -> sorted entries, for context in the report
  egress.dns      sorted normalised names the processes tried to resolve
  egress.phases   phase -> {rc, noroute, unexpected_dns, names}

  rc-profile.py build --trace DIR --egress DIR --release VER --commit SHA -o PROFILE.json
  rc-profile.py diff BASE.json NEW.json [--signoff FILE] [--md OUT.md] [--json OUT.json]
  rc-profile.py --selftest

`diff` exits 1 when NEW adds anything BASE does not have (a trace entry, i.e. a new syscall
class, path, socket kind, connect target or exec; a DNS name; a phase with more noroute
attempts or unexpected names) and no sign-off line covers it; removals are listed but pass, and
the entries in VOLATILE (seen in some runs of the same code only) never need a sign-off.
Sign-off file: one rule per line, "kind<TAB>value-glob<TAB>reason" (kind is a trace kind,
`dns`, or `noroute`/`unexpected` with the phase as value); `*`/`?` are the only wildcards and a
rule without a reason is an error. Exit 2 on bad input.
"""
import argparse
import json
import os
import re
import sys
import tempfile

SCHEMA = 1
NORMALISE = [
    # staging files: .<name>.part-<pid>-<nanos>
    (re.compile(r"\.part-\d+-\d+$"), ".part-*"),
    # system CA store files (hash links, per-CA .pem files, bundles) change with the
    # ca-certificates package, not with our code: keep only the directory
    (re.compile(r"^(/etc/ssl/certs|/etc/pki/tls/certs|/etc/security/certificates)/[^/]+$"), r"\1/*"),
    # the cgroup the process runs in differs by host and runner
    (re.compile(r"^/sys/fs/cgroup/.+/([^/]+)$"), r"/sys/fs/cgroup/*/\1"),
]
NUM_SUFFIX = re.compile(r"-\d+$")
# Entries that some runs of the same code show and others do not. They are reported as
# `volatile` and never need a sign-off, whichever way they change. Keep the reason.
VOLATILE = {
    # glibc malloc (arena.c check_may_shrink_heap) reads it when a thread arena shrinks, which
    # depends on timing: copy-cluster showed it in 2 of 3 runs of the same tree (L-705)
    "path\t/proc/sys/vm/overcommit_memory",
}


def die(msg):
    print("rc-profile: " + msg, file=sys.stderr)
    sys.exit(2)


def norm_value(v):
    for rx, rep in NORMALISE:
        v = rx.sub(rep, v)
    return v


def norm_name(n):
    return NUM_SUFFIX.sub("-N", n.rstrip("."))


def read_trace(d):
    p = os.path.join(d, "observed.tsv")
    if not os.path.isfile(p):
        die("no observed.tsv in %s (run scripts/sup/trace.sh)" % d)
    cases, entries = {}, set()
    with open(p) as f:
        for n, line in enumerate(f, 1):
            parts = line.rstrip("\n").split("\t")
            if len(parts) != 3:
                die("%s:%d: expected case<TAB>kind<TAB>value" % (p, n))
            case, kind, value = parts
            e = "%s\t%s" % (kind, norm_value(value))
            entries.add(e)
            cases.setdefault(case, set()).add(e)
    if not entries:
        die("%s is empty" % p)
    return {"entries": sorted(entries), "cases": {c: sorted(v) for c, v in sorted(cases.items())}}


PHASE = re.compile(r"^EGRESS phase=(\S+) rc=(\S+) unexpected_dns=(\d+) noroute=(\d+) names=\[([^\]]*)\]")


def read_egress(d):
    s, dl = os.path.join(d, "egress-summary.log"), os.path.join(d, "dns.log")
    if not os.path.isfile(s) or not os.path.isfile(dl):
        die("no egress-summary.log/dns.log in %s (run scripts/sup/egress.sh)" % d)
    phases = {}
    with open(s) as f:
        for line in f:
            m = PHASE.match(line)
            if m:
                ph, rc, unexp, noroute, names = m.groups()
                phases[ph] = {
                    "rc": rc,
                    "noroute": int(noroute),
                    "unexpected_dns": int(unexp),
                    "names": sorted({norm_name(x) for x in names.split()}),
                }
    if not phases:
        die("%s has no EGRESS phase lines" % s)
    dns = set()
    with open(dl) as f:
        for line in f:
            p = line.split()
            if len(p) >= 3 and p[0] == "DNS":
                dns.add(norm_name(p[2]))
    return {"dns": sorted(dns), "phases": dict(sorted(phases.items()))}


def build(a):
    prof = {
        "schema": SCHEMA,
        "release": a.release,
        "commit": a.commit,
        "trace": read_trace(a.trace),
        "egress": read_egress(a.egress),
    }
    with open(a.output, "w") as f:
        json.dump(prof, f, indent=1, sort_keys=True)
        f.write("\n")
    print("rc-profile: %s: %d trace entries, %d cases, %d DNS names, %d egress phases"
          % (a.output, len(prof["trace"]["entries"]), len(prof["trace"]["cases"]),
             len(prof["egress"]["dns"]), len(prof["egress"]["phases"])))


def load(p):
    try:
        with open(p) as f:
            prof = json.load(f)
    except (OSError, ValueError) as e:
        die("cannot read %s: %s" % (p, e))
    if prof.get("schema") != SCHEMA:
        die("%s: schema %r, expected %d" % (p, prof.get("schema"), SCHEMA))
    return prof


def glob_re(pattern):
    return re.compile("^" + "".join(".*" if c == "*" else "." if c == "?" else re.escape(c) for c in pattern) + "$")


def load_signoff(p):
    rules = []
    if not p:
        return rules
    with open(p) as f:
        for n, line in enumerate(f, 1):
            line = line.rstrip("\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 3 or not parts[2].strip():
                die("%s:%d: need kind<TAB>value-glob<TAB>reason" % (p, n))
            rules.append((parts[0], glob_re(parts[1]), parts[2].strip(), line))
    return rules


def covered(kind, value, rules):
    for k, rx, reason, _ in rules:
        if k == kind and rx.match(value):
            return reason
    return None


def diff_profiles(base, new, rules):
    """List of (change, kind, value, detail) rows; change is added/removed/increased."""
    rows = []
    b, n = set(base["trace"]["entries"]), set(new["trace"]["entries"])
    for e in sorted(n - b):
        kind, value = e.split("\t", 1)
        cases = [c for c, es in new["trace"]["cases"].items() if e in es]
        rows.append(("added", kind, value, "cases: " + ", ".join(cases)))
    for e in sorted(b - n):
        kind, value = e.split("\t", 1)
        rows.append(("removed", kind, value, ""))
    bd, nd = set(base["egress"]["dns"]), set(new["egress"]["dns"])
    rows += [("added", "dns", x, "") for x in sorted(nd - bd)]
    rows += [("removed", "dns", x, "") for x in sorted(bd - nd)]
    bp = base["egress"]["phases"]
    for ph, cur in new["egress"]["phases"].items():
        old = bp.get(ph, {"noroute": 0, "names": []})
        if cur["noroute"] > old["noroute"]:
            rows.append(("increased", "noroute", ph, "%d -> %d" % (old["noroute"], cur["noroute"])))
        extra = sorted(set(cur["names"]) - set(old["names"]))
        if extra:
            rows.append(("increased", "unexpected", ph, "new names: " + ", ".join(extra)))
    out = []
    for change, kind, value, detail in rows:
        reason = covered(kind, value, rules) if change != "removed" else None
        if "%s\t%s" % (kind, value) in VOLATILE:
            status = "volatile"
        elif change == "removed":
            status = "removed"
        else:
            status = "signed-off" if reason else "NEEDS SIGN-OFF"
        out.append({"change": change, "kind": kind, "value": value, "detail": detail,
                    "status": status, "signoff": reason or ""})
    return out


def report_md(base, new, rows):
    need = [r for r in rows if r["status"] == "NEEDS SIGN-OFF"]
    lines = [
        "# Behaviour profile diff (SUP-016)",
        "",
        "Baseline: %s (%s). Candidate: %s (%s)." % (base["release"], base["commit"], new["release"], new["commit"]),
        "",
        "Trace entries %d -> %d; DNS names %d -> %d; changes %d; needing sign-off %d."
        % (len(base["trace"]["entries"]), len(new["trace"]["entries"]), len(base["egress"]["dns"]),
           len(new["egress"]["dns"]), len(rows), len(need)),
        "",
    ]
    if not rows:
        lines.append("No differences.")
    else:
        lines += ["| status | change | kind | value | detail |", "|---|---|---|---|---|"]
        for r in rows:
            detail = r["detail"] + ("; sign-off: " + r["signoff"] if r["signoff"] else "")
            lines.append("| %s | %s | %s | `%s` | %s |" % (r["status"], r["change"], r["kind"],
                                                         r["value"].replace("|", "\\|"), detail.replace("|", "\\|")))
    return "\n".join(lines) + "\n"


def diff(a):
    base, new = load(a.base), load(a.new)
    rows = diff_profiles(base, new, load_signoff(a.signoff))
    md = report_md(base, new, rows)
    if a.md:
        with open(a.md, "w") as f:
            f.write(md)
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"base": a.base, "new": a.new, "changes": rows}, f, indent=1)
            f.write("\n")
    need = [r for r in rows if r["status"] == "NEEDS SIGN-OFF"]
    for r in rows[:40]:
        print("%-14s %-9s %-10s %s %s" % (r["status"], r["change"], r["kind"], r["value"], r["detail"]))
    if len(rows) > 40:
        print("... %d more (see the report)" % (len(rows) - 40))
    print("RC_DIFF changes=%d needs_signoff=%d %s" % (len(rows), len(need), "FAIL" if need else "OK"))
    return 1 if need else 0


def selftest():
    def prof(entries, dns, phases):
        cases = {"c1": sorted(entries)}
        return {"schema": SCHEMA, "release": "t", "commit": "t",
                "trace": {"entries": sorted(entries), "cases": cases},
                "egress": {"dns": sorted(dns), "phases": phases}}
    ph0 = {"T1-unit": {"rc": "0", "noroute": 0, "unexpected_dns": 0, "names": []}}
    base = prof(["exec\t@BIN", "path\t/etc/hosts", "syscall\texit"], ["no-such-host.invalid"], ph0)
    ok = True

    def case(name, new, rules, want_need, want_rows=None):
        nonlocal ok
        rows = diff_profiles(base, new, rules)
        need = sum(r["status"] == "NEEDS SIGN-OFF" for r in rows)
        good = need == want_need and (want_rows is None or len(rows) == want_rows)
        ok &= good
        print("selftest %-22s: %s (changes %d, needs sign-off %d)" % (name, "OK" if good else "FAIL", len(rows), need))
    case("identical", base, [], 0, 0)
    case("new-syscall", prof(base["trace"]["entries"] + ["syscall\tptrace"], base["egress"]["dns"], ph0), [], 1)
    case("new-path", prof(base["trace"]["entries"] + ["path\t/root/.ssh/id_rsa"], base["egress"]["dns"], ph0), [], 1)
    case("new-exec", prof(base["trace"]["entries"] + ["exec\t/bin/sh"], base["egress"]["dns"], ph0), [], 1)
    case("new-connect", prof(base["trace"]["entries"] + ["connect\t203.0.113.7:443"], base["egress"]["dns"], ph0), [], 1)
    case("new-dns", prof(base["trace"]["entries"], base["egress"]["dns"] + ["crates.io"], ph0), [], 1)
    ph1 = {"T1-unit": {"rc": "0", "noroute": 2, "unexpected_dns": 1, "names": ["example.com"]}}
    case("noroute+unexpected", prof(base["trace"]["entries"], base["egress"]["dns"], ph1), [], 2)
    case("removed-only", prof(["exec\t@BIN"], [], ph0), [], 0, 3)
    rules = [("path", glob_re("/etc/new/*"), "new config dir, reviewed", "")]
    case("volatile", prof(base["trace"]["entries"] + ["path\t/proc/sys/vm/overcommit_memory"], base["egress"]["dns"], ph0), [], 0, 1)
    case("signed-off", prof(base["trace"]["entries"] + ["path\t/etc/new/a.conf"], base["egress"]["dns"], ph0), rules, 0, 1)
    case("sign-off-wrong-kind", prof(base["trace"]["entries"] + ["exec\t/etc/new/a.conf"], base["egress"]["dns"], ph0), rules, 1)
    # normalisation: run-specific names map to the same entry
    norm_ok = (norm_value("@WORK/out/.x.json.part-123-456") == "@WORK/out/.x.json.part-*"
               and norm_value("/etc/ssl/certs/0a775a30.0") == "/etc/ssl/certs/*"
               and norm_value("/etc/ssl/certs/ISRG_Root_X1.pem") == "/etc/ssl/certs/*"
               and norm_value("/etc/ssl/certs") == "/etc/ssl/certs"
               and norm_value("/sys/fs/cgroup/agent/memory.max") == "/sys/fs/cgroup/*/memory.max"
               and norm_value("/sys/fs/cgroup/memory.max") == "/sys/fs/cgroup/memory.max"
               and norm_name("elasticdump-rs-egress-1262826") == "elasticdump-rs-egress-N")
    ok &= norm_ok
    print("selftest %-22s: %s" % ("normalisation", "OK" if norm_ok else "FAIL"))
    # a sign-off rule without a reason is refused
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
        f.write("path\t/etc/*\t\n")
    try:
        load_signoff(f.name)
        refused = False
    except SystemExit as e:
        refused = e.code == 2
    finally:
        os.unlink(f.name)
    ok &= refused
    print("selftest %-22s: %s" % ("signoff-needs-reason", "OK" if refused else "FAIL"))
    print("RC_PROFILE_SELFTEST %s" % ("OK" if ok else "FAIL"))
    return 0 if ok else 1


def main():
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("--trace", required=True)
    b.add_argument("--egress", required=True)
    b.add_argument("--release", required=True)
    b.add_argument("--commit", required=True)
    b.add_argument("-o", "--output", required=True)
    d = sub.add_parser("diff")
    d.add_argument("base")
    d.add_argument("new")
    d.add_argument("--signoff")
    d.add_argument("--md")
    d.add_argument("--json")
    a = ap.parse_args()
    if a.cmd == "build":
        build(a)
        return 0
    return diff(a)


if __name__ == "__main__":
    sys.exit(main())
