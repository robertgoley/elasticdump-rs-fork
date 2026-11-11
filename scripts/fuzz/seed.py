#!/usr/bin/env python3
"""Seed fuzz/corpus/<target>/ from the golden recordings and Node fixtures (TST-API-084).

    scripts/fuzz/seed.py            # (re)creates fuzz/corpus/*; the corpus is not committed
"""
import hashlib, json, pathlib, shutil

root = pathlib.Path(__file__).resolve().parents[2]
corpus = root / "fuzz" / "corpus"
seeds = {t: [] for t in [
    "json_split", "codec_decode", "bulk_encode_doc", "extract_batch", "bulk_handle_response",
    "parse_size", "endpoint_parse", "ilm_to_ism_convert", "renamer_new_index", "ini_auth_file",
    "zstd_in", "gzip_in", "sonic_vs_serde",
    "file_to_bulk", "url_segments", "meta_names", "secret_redaction"]}

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
seeds["secret_redaction"] += [b"fakeuser:FAKE-pw: @%#/?", "u:FAKE-\u00e9 \\\"x".encode(), b"u:x***"]
seeds["ini_auth_file"] += [b"user=u\npassword=p\n", b"user = a\r\npassword = b c\r\n", b"[s]\nuser=x",
                           b"password=\nuser=\n"]

# compressed input (SUP-020): gzip members and zstd frames, whole, concatenated, truncated,
# with trailing garbage; zstd frames come from the release binary (--compressionType zstd)
# when it exists, plus hand-made raw/RLE-block frames.
import gzip, struct, subprocess, tempfile
plain = (b'{"_id":"a","_source":{"n":1}}\n{"_id":"b","_source":{"s":"\\u00e9"}}\n')
g = gzip.compress(plain, 1)
seeds["gzip_in"] += [g, g + g, g[:-5], g + b"junk", gzip.compress(b" " * 100000, 9)]
def frame(wd, blocks):
    f = b"\x28\xb5\x2f\xfd\x00" + bytes([wd])
    for i, (ty, size, body) in enumerate(blocks):
        h = int(i + 1 == len(blocks)) | (ty << 1) | (size << 3)
        f += struct.pack("<I", h)[:3] + body
    return f
raw = frame(0x38, [(0, len(plain), plain)])
rle = frame(0x38, [(0, 5, b'{"v":'), (1, 4096, b"7"), (0, 2, b"}\n")])
skip = b"\x50\x2a\x4d\x18\x04\x00\x00\x00junk"
seeds["zstd_in"] += [raw, rle, raw + skip + rle, raw[:-3], raw + b"junk", frame(0xa8, [(0, 1, b"{")])]
binp = root / "target" / "release" / "searchdump"
if binp.exists():
    with tempfile.TemporaryDirectory() as td:
        src = pathlib.Path(td) / "in.json"
        src.write_bytes(plain * 20)
        out = pathlib.Path(td) / "out.json"
        r = subprocess.run([str(binp), "--input", str(src), "--output", str(out), "--compressionType", "zstd"],
                           capture_output=True)
        for z in sorted(pathlib.Path(td).glob("out.json*")):
            if not z.read_bytes().startswith(b"\x28\xb5\x2f\xfd"):
                continue
            b = z.read_bytes()
            seeds["zstd_in"] += [b, b + b, b[:-4]]

seeds["sonic_vs_serde"] += seeds["bulk_encode_doc"][:200] + seeds["extract_batch"][:50] + [
    b'{"a":1,"a":2}', b'[1,"x",{"b":[]}]', b'{"s":"\\ud800"}', b'{"n":1e400,"m":-0.0}', b'{"a":1} x',
    b'{"k":"\\u00e9\\/\\""}']

seeds["file_to_bulk"] += [b"\n".join(seeds["bulk_encode_doc"][i:i + 5]) for i in range(0, 100, 5)]
seeds["url_segments"] += [b"logs\x00_search", b"a b\x00%2e\x00..", "é\x00😀".encode(), b"lo\ngs\x00_mapping"]
seeds["meta_names"] += [b"logs-2024", b".kibana", b"_all", "ǅ".encode(), b"a:b", b"x" * 256]

shutil.rmtree(corpus, ignore_errors=True)
for t, items in seeds.items():
    d = corpus / t
    d.mkdir(parents=True)
    for item in items:
        (d / hashlib.sha1(item).hexdigest()).write_bytes(item)
    print(f"{t}: {len(list(d.iterdir()))} seeds")
