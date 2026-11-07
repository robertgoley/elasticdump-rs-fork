#!/usr/bin/env python3
"""Tiny in-memory stand-in for an Elasticsearch/OpenSearch node, for the
multielasticdump round-trip test (scripts/it/multielasticdump-roundtrip.sh).

Covers only what elasticdump-rs settings/mapping/alias/data runs and
multielasticdump call: /, _cat/indices, index create/get/delete, _settings,
_mapping, _aliases, _search + scroll, _bulk, _refresh, _count. Not a cluster.
Usage: fake_es.py PORT [seed] [os]   (seed: sample indices; os: report OpenSearch 2.17)
Listens on 127.0.0.1 only. Unhandled requests are logged to stderr as UNHANDLED.
"""
import json, sys, re, threading, uuid
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs, unquote
IDX = {}  # name -> {settings, mappings, aliases, docs:[(id,src)]}
SCROLLS = {}
LOCK = threading.Lock()
def new_idx(): return {"settings": {"index": {"number_of_shards": "1", "number_of_replicas": "0", "uuid": "x", "creation_date": "1", "provided_name": "x", "version": {"created": "8"}}}, "mappings": {}, "aliases": {}, "docs": {}}
class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def send(self, code, obj):
        b = json.dumps(obj, separators=(",",":")).encode() if not isinstance(obj, bytes) else obj
        self.send_response(code); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(b)))
        self.end_headers()
        if self.command != "HEAD": self.wfile.write(b)
    def body(self):
        n = int(self.headers.get("content-length") or 0)
        if self.headers.get("transfer-encoding","").lower()=="chunked":
            data=b""
            while True:
                l=int(self.rfile.readline().strip(),16)
                if l==0: self.rfile.readline(); break
                data+=self.rfile.read(l); self.rfile.readline()
            return data
        return self.rfile.read(n) if n else b""
    def do_HEAD(self): self.route("HEAD")
    def do_GET(self): self.route("GET")
    def do_PUT(self): self.route("PUT")
    def do_POST(self): self.route("POST")
    def do_DELETE(self): self.route("DELETE")
    def route(self, m):
        u = urlparse(self.path); q = parse_qs(u.query); parts = [unquote(p) for p in u.path.split("/") if p]
        raw = self.body()
        with LOCK:
            try: r = self.handle_req(m, parts, q, raw)
            except Exception as e: r = (500, {"error": repr(e)})
        if r is None:
            sys.stderr.write(f"UNHANDLED {m} {self.path}\n"); r = (400, {"error": "unhandled "+self.path})
        sys.stderr.write(f"{m} {self.path} -> {r[0]}\n")
        self.send(*r)
    def handle_req(self, m, p, q, raw):
        j = lambda: json.loads(raw) if raw.strip() else {}
        if not p: return (200, {"name":"fake","cluster_name":"fake","version":({"number":"2.17.0","distribution":"opensearch"} if OS else {"number":"8.15.0","build_flavor":"default"}),"tagline":"You Know, for Search"})
        if p[0] == "_cat" and p[1] == "indices":
            return (200, [{"index": k, "status": "open", "health":"green", "docs.count": str(len(v["docs"]))} for k, v in sorted(IDX.items())])
        if p[0] in ("_search",) and len(p) > 1 and p[1] == "scroll":
            sid = (j().get("scroll_id") if raw.strip() else None) or q.get("scroll_id", [None])[0]
            if m == "DELETE": SCROLLS.pop(sid, None); return (200, {"succeeded": True, "num_freed": 1})
            return self.page(sid)
        if p[0] == "_bulk" or (len(p) == 2 and p[1] == "_bulk"):
            lines = [l for l in raw.decode().split("\n") if l.strip()]; items = []
            i = 0
            while i < len(lines):
                a = json.loads(lines[i]); op = next(iter(a)); meta = a[op]
                name = meta.get("_index") or p[0]; did = meta.get("_id") or uuid.uuid4().hex
                src = json.loads(lines[i+1]) if op != "delete" else None; i += 2 if op != "delete" else 1
                ix = IDX.setdefault(name, new_idx()); ix["docs"][did] = src
                items.append({op: {"_index": name, "_id": did, "status": 201, "result": "created"}})
            return (200, {"took": 1, "errors": False, "items": items})
        if p[0] == "_aliases" and m == "POST":
            for a in j().get("actions", []):
                op = next(iter(a)); d = dict(a[op]); n = d.pop("index"); al = d.pop("alias")
                if op == "add": IDX[n]["aliases"][al] = d
                else: IDX[n]["aliases"].pop(al, None)
            return (200, {"acknowledged": True})
        name = p[0]
        if name.startswith("_"): return None
        names = [n for n in IDX if re.fullmatch(name.replace(".", r"\.").replace("*", ".*"), n)] if ("*" in name) else ([name] if name in IDX else [])
        if len(p) == 1:
            if m == "HEAD": return (200 if names else 404, b"")
            if m == "PUT":
                if name in IDX: return (400, {"error":{"root_cause":[{"type":"resource_already_exists_exception","reason":"index ["+name+"/abc] already exists","index":name}],"type":"resource_already_exists_exception","reason":"index ["+name+"/abc] already exists","index":name},"status":400})
                b = j(); ix = new_idx(); s = b.get("settings", {})
                ix["settings"]["index"].update(s.get("index", s)); ix["mappings"] = b.get("mappings", {}); ix["aliases"] = b.get("aliases", {})
                IDX[name] = ix; return (200, {"acknowledged": True, "index": name})
            if m == "DELETE": IDX.pop(name, None); return (200, {"acknowledged": True})
            if m == "GET":
                if not names: return (404, {"error":{"type":"index_not_found_exception"},"status":404})
                return (200, {n: {"settings": IDX[n]["settings"], "mappings": IDX[n]["mappings"], "aliases": IDX[n]["aliases"]} for n in names})
        op = p[1]
        if op in ("_settings","_mapping","_mappings","_alias","_aliases","_count","_search","_refresh") and not names and m != "PUT" and not (op=="_search" and m=="POST" and False):
            return (404, {"error":{"type":"index_not_found_exception","reason":"no such index ["+name+"]"},"status":404})
        if op == "_settings":
            if m == "GET": return (200, {n: {"settings": IDX[n]["settings"]} for n in names})
            if m == "PUT": IDX[name]["settings"]["index"].update(j().get("index", j())); return (200, {"acknowledged": True})
        if op in ("_mapping", "_mappings"):
            if m == "GET": return (200, {n: {"mappings": IDX[n]["mappings"]} for n in names})
            if m == "PUT":
                if name not in IDX: return (404, {"error":{"type":"index_not_found_exception"},"status":404})
                IDX[name]["mappings"].update(j()); return (200, {"acknowledged": True})
        if op in ("_alias", "_aliases"):
            if m == "GET": return (200, {n: {"aliases": IDX[n]["aliases"]} for n in names})
            if m in ("PUT","POST") and len(p) == 3: IDX[name]["aliases"][p[2]] = j(); return (200, {"acknowledged": True})
        if op == "_refresh": return (200, {"_shards": {"total": 1, "successful": 1, "failed": 0}})
        if op == "_count": return (200, {"count": sum(len(IDX[n]["docs"]) for n in names), "_shards": {"total":1,"successful":1,"failed":0}})
        if op == "_search":
            b = j(); size = int(q.get("size", [b.get("size", 10)])[0])
            docs = [(n, i, s) for n in names for i, s in IDX[n]["docs"].items()]
            sid = uuid.uuid4().hex; SCROLLS[sid] = {"docs": docs, "pos": 0, "size": size}
            return self.page(sid)
        return None
    def page(self, sid):
        s = SCROLLS.get(sid)
        if s is None: return (404, {"error":{"type":"search_context_missing_exception"},"status":404})
        chunk = s["docs"][s["pos"]:s["pos"]+s["size"]]; s["pos"] += len(chunk)
        return (200, {"_scroll_id": sid, "took": 1, "timed_out": False, "_shards":{"total":1,"successful":1,"skipped":0,"failed":0},
                      "hits": {"total": {"value": len(s["docs"]), "relation": "eq"}, "max_score": 1.0, "hits": [{"_index": n, "_id": i, "_score": 1.0, "_source": src} for n, i, src in chunk]}})
port = int(sys.argv[1]); OS = "os" in sys.argv
if "seed" in sys.argv:
    # seed
    for name, cnt in [("logs-app-2026.10.01", 7), ("logs-app-2026.10.02", 3), ("logs-web-2026.10.01", 5), ("metrics-1", 2), (".kibana_1", 1), ("empty-logs-x", 0)]:
        ix = new_idx(); ix["mappings"] = {"properties": {"msg": {"type": "text"}, "n": {"type": "long"}}}
        ix["settings"]["index"]["lifecycle"] = {"name": "logs-policy"}
        if name == "logs-app-2026.10.01": ix["aliases"] = {"logs-app": {}}
        for k in range(cnt): ix["docs"][f"d{k}"] = {"msg": f"{name} doc {k}", "n": k}
        IDX[name] = ix
ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
