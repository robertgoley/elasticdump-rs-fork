#!/usr/bin/env python3
"""Read the front matter of a task card (restricted YAML subset, stdlib only).

  card.py get  <card> <key>        scalar value ('' if missing)
  card.py list <card> <key>        one list item per line
  card.py acceptance <card>        one line per item: <exit>\t<run>
  card.py check <card>             validate required fields; exit 1 on error
  card.py body <card>              the markdown after the front matter

Supported syntax between the first two '---' lines:
  key: value            key: [a, b]           key:
                                               - item
  acceptance:
    - run: <command>
      exit: <int>
      expect: <text>
"""
import re
import sys

RAW_KEYS = ("campaign", "acceptance", "goal", "forbidden")


def strip_comment(key, v):
    return v if key in RAW_KEYS else re.sub(r"\s+#.*$", "", v)

REQUIRED = ["id", "lane", "agent", "requirements", "goal", "files_allowed", "forbidden",
            "branch", "summary",
            "acceptance", "timeout_min"]


def parse(path):
    lines = open(path, encoding="utf-8").read().split("\n")
    if not lines or lines[0].strip() != "---":
        sys.exit(f"{path}: no front matter")
    end = next((i for i in range(1, len(lines)) if lines[i].strip() == "---"), None)
    if end is None:
        sys.exit(f"{path}: unterminated front matter")
    data, key, item = {}, None, None
    for raw in lines[1:end]:
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        ind = len(raw) - len(raw.lstrip())
        s = raw.strip()
        if ind == 0:
            k, _, v = s.partition(":")
            key = k.strip()
            v = strip_comment(key, v.strip())
            if v.startswith("[") and v.endswith("]"):
                data[key] = [x.strip() for x in v[1:-1].split(",") if x.strip()]
            elif v:
                data[key] = v
            else:
                data[key] = []
            item = None
        elif s.startswith("- "):
            v = s[2:]
            if key == "acceptance":
                k, _, val = v.partition(":")
                item = {k.strip(): val.strip()}
                data[key].append(item)
            else:
                data[key].append(strip_comment(key, v.strip()))
        elif key == "acceptance" and item is not None:
            k, _, val = s.partition(":")
            item[k.strip()] = val.strip()
    body = "\n".join(lines[end + 1:])
    return data, body


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    cmd, path = sys.argv[1], sys.argv[2]
    d, body = parse(path)
    if cmd == "get":
        v = d.get(sys.argv[3], "")
        print(v if isinstance(v, str) else " ".join(v))
    elif cmd == "list":
        v = d.get(sys.argv[3], [])
        for x in ([v] if isinstance(v, str) and v else v):
            print(x)
    elif cmd == "acceptance":
        for a in d.get("acceptance", []):
            print(f"{a.get('exit', '0')}\t{a.get('run', '')}")
    elif cmd == "check":
        bad = [k for k in REQUIRED if not d.get(k)]
        if d.get("lane") not in ("code", "campaign"):
            bad.append("lane (code|campaign)")
        if d.get("agent") not in ("impl", "runner", "lead"):
            bad.append("agent (impl|runner|lead)")
        if d.get("lane") == "campaign" and not d.get("campaign"):
            bad.append("campaign (required for lane: campaign)")
        for a in d.get("acceptance", []):
            if not a.get("run"):
                bad.append("acceptance item without run")
        if bad:
            sys.exit(f"{path}: missing/invalid: {', '.join(bad)}")
        print(f"{path}: ok ({d['id']})")
    elif cmd == "body":
        print(body)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
