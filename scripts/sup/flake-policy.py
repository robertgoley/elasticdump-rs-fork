#!/usr/bin/env python3
"""INF-013 flake policy gate for .config/nextest.toml.

Rules:
  * correctness tests never auto-retry: every profile and override has retries == 0
    (or no retries key);
  * a quarantined test is listed in nextest.toml as a comment line
        # quarantine: filter="test(=name)" owner="@someone" since="YYYY-MM-DD" reason="..."
    and every field is required;
  * a quarantine older than 14 days fails CI.

  flake-policy.py            check (exit 1 on violation)
  flake-policy.py --filter   print a nextest -E expression excluding the quarantined tests
                             (prints `all()` when the list is empty)
"""
import datetime as dt
import re
import sys
import tomllib
from pathlib import Path

MAX_AGE_DAYS = 14
CONF = Path(__file__).resolve().parents[2] / ".config" / "nextest.toml"
LINE = re.compile(r'^#\s*quarantine:\s*(.*)$')
FIELD = re.compile(r'(\w+)="([^"]*)"')


def quarantine(text):
    out, errs = [], []
    for n, line in enumerate(text.splitlines(), 1):
        m = LINE.match(line.strip())
        if not m:
            continue
        f = dict(FIELD.findall(m.group(1)))
        missing = [k for k in ("filter", "owner", "since", "reason") if not f.get(k)]
        if missing:
            errs.append(f"line {n}: quarantine entry missing {', '.join(missing)}")
            continue
        try:
            f["since_date"] = dt.date.fromisoformat(f["since"])
        except ValueError:
            errs.append(f"line {n}: bad since date {f['since']!r}")
            continue
        f["line"] = n
        out.append(f)
    return out, errs


def retries_violations(conf):
    bad = []
    for name, prof in conf.get("profile", {}).items():
        r = prof.get("retries", 0)
        if r not in (0, None) and r != {"backoff": "fixed", "count": 0}:
            bad.append(f"profile.{name}: retries = {r!r} (correctness tests never auto-retry)")
        for i, ov in enumerate(prof.get("overrides", [])):
            if ov.get("retries", 0) not in (0, None):
                bad.append(f"profile.{name}.overrides[{i}] ({ov.get('filter')}): retries = {ov['retries']!r}")
    return bad


def main(argv):
    text = CONF.read_text()
    q, errs = quarantine(text)
    if "--filter" in argv:
        print("not (" + " | ".join(e["filter"] for e in q) + ")" if q else "all()")
        return 0
    today = dt.date.fromisoformat(argv[argv.index("--today") + 1]) if "--today" in argv else dt.date.today()
    errs += retries_violations(tomllib.loads(text))
    for e in q:
        age = (today - e["since_date"]).days
        if age > MAX_AGE_DAYS:
            errs.append(f"line {e['line']}: {e['filter']} quarantined {age} days (> {MAX_AGE_DAYS}; owner {e['owner']}): fix or delete it")
    for e in errs:
        print("flake-policy:", e, file=sys.stderr)
    if not errs:
        print(f"flake-policy: ok ({len(q)} quarantined, retries 0 everywhere)")
    return 1 if errs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
