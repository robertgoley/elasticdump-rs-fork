#!/usr/bin/env python3
"""Tiny in-memory stand-in for an Elasticsearch/OpenSearch node, for the
multielasticdump round-trip test (scripts/it/multielasticdump-roundtrip.sh).

Covers only what elasticdump-rs settings/analyzer/mapping/alias/data and
template/component_template/index_template/policy/script runs and
multielasticdump call: /, _cat/indices, _cat/aliases, index create/get/delete,
_settings, _mapping, _aliases, _search + scroll, _bulk, _refresh, _count,
_data_stream (a bulk create into a name that an index template with
data_stream matches creates the stream), _template, _component_template,
_index_template, _ilm/policy, _plugins/_ism/policies, _scripts and
_cluster/state/metadata. Not a cluster.
Usage: fake_es.py PORT [seed] [os]   (seed: sample indices; os: report OpenSearch 2.17)
Listens on 127.0.0.1 only. Unhandled requests are logged to stderr as UNHANDLED.
"""
import json, sys, re, threading, uuid
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs, unquote
IDX = {}  # name -> {settings, mappings, aliases, docs:[(id,src)]}
SCROLLS = {}
DS = {}  # data stream -> [backing index]
TPL = {"_template": {}, "_component_template": {}, "_index_template": {}}
ILM = {}; ISM = {}; SCRIPTS = {}
def ds_template(name):
    for t in TPL["_index_template"].values():
        if "data_stream" in t and any(re.fullmatch(pat.replace(".", r"\.").replace("*", ".*"), name) for pat in t.get("index_patterns", [])):
            return t
    return None
def make_ds(name):
    b = f".ds-{name}-2026.11.07-000001"; ix = new_idx(); ix["settings"]["index"]["hidden"] = "true"
    t = ds_template(name) or {}; ix["mappings"] = t.get("template", {}).get("mappings", {})
    IDX[b] = ix; DS[name] = [b]; return b
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
                if name not in DS and name not in IDX and ds_template(name): make_ds(name)
                if name in DS:
                    if op != "create":
                        items.append({op: {"_index": name, "_id": did, "status": 400, "error": {"type": "illegal_argument_exception", "reason": "only write ops with an op_type of create are allowed in data streams"}}}); continue
                    name = DS[name][-1]
                ix = IDX.setdefault(name, new_idx()); ix["docs"][did] = src
                items.append({op: {"_index": name, "_id": did, "status": 201, "result": "created"}})
            return (200, {"took": 1, "errors": any("error" in next(iter(x.values())) for x in items), "items": items})
        if p[0] == "_aliases" and m == "POST":
            for a in j().get("actions", []):
                op = next(iter(a)); d = dict(a[op]); n = d.pop("index"); al = d.pop("alias")
                if op == "add": IDX[n]["aliases"][al] = d
                else: IDX[n]["aliases"].pop(al, None)
            return (200, {"acknowledged": True})
        r = self.cluster_req(m, p, q, j)
        if r is not None: return r
        name = p[0]
        if name.startswith("_"): return None
        names = [n for n in IDX if re.fullmatch(name.replace(".", r"\.").replace("*", ".*"), n)] if ("*" in name) else ([name] if name in IDX else list(DS.get(name, [])))
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
    def cluster_req(self, m, p, q, j):
        """Cluster-scoped endpoints; None when the path is not one of them."""
        if p[:2] == ["_cat", "aliases"]:
            return (200, [{"alias": a, "index": n} for n, v in sorted(IDX.items()) for a in sorted(v["aliases"])])
        if p[0] == "_data_stream":
            if m == "GET":
                want = [n for n in sorted(DS) if len(p) < 2 or n == p[1]]
                if len(p) > 1 and not want: return (404, {"error": {"type": "index_not_found_exception"}, "status": 404})
                return (200, {"data_streams": [{"name": n, "timestamp_field": {"name": "@timestamp"}, "indices": [{"index_name": b, "index_uuid": "x"} for b in DS[n]], "generation": len(DS[n]), "status": "GREEN", "template": next((k for k, t in TPL["_index_template"].items() if t is ds_template(n)), "")} for n in want]})
            if m == "PUT" and len(p) == 2:
                if not ds_template(p[1]): return (400, {"error": {"type": "index_template_missing_exception", "reason": "no matching index template found for data stream [" + p[1] + "]"}, "status": 400})
                make_ds(p[1]); return (200, {"acknowledged": True})
        if p[0] in TPL:
            store = TPL[p[0]]
            if m == "GET":
                want = {k: v for k, v in sorted(store.items()) if len(p) < 2 or re.fullmatch(p[1].replace("*", ".*"), k)}
                if len(p) > 1 and not want: return (404, {"error": {"type": "resource_not_found_exception"}, "status": 404})
                if p[0] == "_template": return (200, want)
                key = "component_template" if p[0] == "_component_template" else "index_template"
                return (200, {key + "s": [{"name": k, key: v} for k, v in want.items()]})
            if m in ("PUT", "POST") and len(p) == 2:
                if "create" in q and q["create"][0] == "true" and p[1] in store: return (400, {"error": {"type": "illegal_argument_exception", "reason": "template [" + p[1] + "] already exists"}, "status": 400})
                for c in j().get("composed_of", []):
                    if c not in TPL["_component_template"]: return (400, {"error": {"type": "invalid_index_template_exception", "reason": "component template [" + c + "] does not exist"}, "status": 400})
                store[p[1]] = j(); return (200, {"acknowledged": True})
            if m == "DELETE" and len(p) == 2: store.pop(p[1], None); return (200, {"acknowledged": True})
        if p[:2] == ["_ilm", "policy"]:
            if OS: return (400, {"error": {"type": "invalid_index_name_exception", "reason": "no such endpoint on OpenSearch"}, "status": 400})
            if m == "GET":
                want = {k: v for k, v in sorted(ILM.items()) if len(p) < 3 or k == p[2]}
                if len(p) > 2 and not want: return (404, {"error": {"type": "resource_not_found_exception"}, "status": 404})
                return (200, {k: {"version": 1, "modified_date": "2026-11-07T00:00:00.000Z", "policy": v} for k, v in want.items()})
            if m == "PUT" and len(p) == 3: ILM[p[2]] = j().get("policy", {}); return (200, {"acknowledged": True})
        if p[:3] == ["_plugins", "_ism", "policies"]:
            if m == "GET":
                if len(p) > 3:
                    if p[3] not in ISM: return (404, {"error": {"type": "status_exception", "reason": "Policy not found"}, "status": 404})
                    return (200, {"_id": p[3], "_version": 1, "_seq_no": 0, "_primary_term": 1, "policy": ISM[p[3]]})
                return (200, {"policies": [{"_id": k, "_seq_no": 0, "_primary_term": 1, "policy": v} for k, v in sorted(ISM.items())], "total_policies": len(ISM)})
            if m == "PUT" and len(p) == 4:
                if p[3] in ISM and "if_seq_no" not in q: return (409, {"error": {"type": "version_conflict_engine_exception", "reason": "policy exists"}, "status": 409})
                pol = j().get("policy", {}); pol["policy_id"] = p[3]; ISM[p[3]] = pol
                return (201, {"_id": p[3], "_version": 1, "_primary_term": 1, "_seq_no": 0, "policy": {"policy": pol}})
        if p[:3] == ["_cluster", "state", "metadata"]:
            return (200, {"cluster_name": "fake", "metadata": {"stored_scripts": dict(sorted(SCRIPTS.items()))}})
        if p[0] == "_scripts" and len(p) >= 2:
            if m in ("PUT", "POST"): SCRIPTS[p[1]] = j().get("script", {}); return (200, {"acknowledged": True})
            if m == "GET":
                if p[1] not in SCRIPTS: return (404, {"_id": p[1], "found": False})
                return (200, {"_id": p[1], "found": True, "script": SCRIPTS[p[1]]})
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
    IDX["metrics-1"]["aliases"] = {"grp-metrics": {}}
    IDX["logs-web-2026.10.01"]["settings"]["index"]["analysis"] = {"analyzer": {"lower": {"type": "custom", "tokenizer": "standard", "filter": ["lowercase"]}}}
    TPL["_component_template"]["base"] = {"template": {"settings": {"index": {"number_of_shards": "1"}}, "mappings": {"properties": {"@timestamp": {"type": "date"}}}}}
    TPL["_index_template"]["web-tpl"] = {"index_patterns": ["web*"], "data_stream": {}, "composed_of": ["base"], "priority": 200, "template": {"settings": {"index": {"lifecycle": {"name": "logs-policy"}}}, "mappings": {"properties": {"msg": {"type": "text"}}}}}
    TPL["_template"]["old-tpl"] = {"index_patterns": ["old-*"], "order": 0, "settings": {"index": {"number_of_shards": "1"}}, "mappings": {}, "aliases": {}}
    ILM["logs-policy"] = {"phases": {"hot": {"min_age": "0ms", "actions": {"rollover": {"max_age": "1d"}}}, "delete": {"min_age": "30d", "actions": {"delete": {}}}}}
    SCRIPTS["add-one"] = {"lang": "painless", "source": "ctx._source.n += 1"}
    b = make_ds("web")
    for k in range(4): IDX[b]["docs"][f"w{k}"] = {"@timestamp": f"2026-11-0{k+1}T00:00:00Z", "msg": f"web doc {k}"}
ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
