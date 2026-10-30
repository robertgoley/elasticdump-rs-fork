#!/usr/bin/env python3
"""Seed fuzz/corpus/<target>/ from the golden recordings and Node fixtures (TST-API-084).

    scripts/fuzz/seed.py            # (re)creates fuzz/corpus/*; the corpus is not committed
"""
import hashlib, json, pathlib, shutil

root = pathlib.Path(__file__).resolve().parents[2]
corpus = root / "fuzz" / "corpus"
seeds = {t: [] for t in [
    "json_split", "codec_decode", "bulk_encode_doc", "extract_batch", "bulk_handle_response",
    "parse_size", "endpoint_parse", "ilm_to_ism_convert", "renamer_new_index", "ini_auth_file"]}

for meta in (root / "tests" / "golden").glob("*/*/meta.toml"):
    case = meta.parent
    for resp in sorted(case.glob("*.resp.json")):
        req = json.loads(resp.with_name(resp.name.replace(".resp", ".req")).read_text())
        body = json.loads(resp.read_text()).get("body")
        if body is None:
            continue
        raw = json.dumps(body).encode()
        path = req.get("path", "")
        if "_bulk" in path:
            seeds["bulk_handle_response"].append(raw)
        elif "_search" in path:
            seeds["extract_batch"].append(raw)
        elif "_ilm/policy" in path:
            seeds["ilm_to_ism_convert"].append(raw)
    for f in ("input.ndjson", "expected.out"):
        p = case / f
        if p.exists():
            data = p.read_bytes()[:64 << 10]
            seeds["json_split"].append(b"\x07" + data)
            seeds["bulk_encode_doc"].extend(data.splitlines()[:20])
for f in (root / "tests" / "fixtures" / "node").glob("*/*.json"):
    for line in f.read_bytes().splitlines()[:5]:
        seeds["codec_decode"].append(line)
seeds["parse_size"] += [b"10mb", b"1.5gb", b"0", b"+1kb", b"100", b"1e3b", b"9999999999999pb"]
seeds["endpoint_parse"] += [b"http://u:p@h:9200/i/t", b"https://[::1]:9200/", b"$", b"/tmp/x.json",
                            b"elastic:pw@host:9200/idx", b"http://a:/pw@h", b"HTTP://H/I"]
seeds["renamer_new_index"] += [b"^(a|b)-(.*):x-$2\na-1\nb-1", b"logs-:new-\nlogs-1\nother",
                               b"(?:x):${1}\nx", b"^(.*)$:_$1\nabc"]
seeds["ini_auth_file"] += [b"user=u\npassword=p\n", b"user = a\r\npassword = b c\r\n", b"[s]\nuser=x",
                           b"password=\nuser=\n"]

shutil.rmtree(corpus, ignore_errors=True)
for t, items in seeds.items():
    d = corpus / t
    d.mkdir(parents=True)
    for item in items:
        (d / hashlib.sha1(item).hexdigest()).write_bytes(item)
    print(f"{t}: {len(list(d.iterdir()))} seeds")
