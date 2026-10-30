#!/usr/bin/env python3
# Minimal ES 8.19 stand-in that appends every _bulk body (POST or PUT, gzip or chunked)
# to a file and acknowledges each item (L-506 differential; usage: recorder.py PORT FILE [v]).
import http.server, json, sys, gzip
out = open(sys.argv[2], 'ab')
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *a): pass
    def _send(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header('content-type', 'application/json')
        self.send_header('x-elastic-product', 'Elasticsearch')
        self.send_header('content-length', str(len(b)))
        self.end_headers()
        if self.command != 'HEAD': self.wfile.write(b)
    def _body(self):
        if self.headers.get('transfer-encoding', '').lower() == 'chunked':
            b = b''
            while True:
                n = int(self.rfile.readline().strip().split(b';')[0], 16)
                if n == 0:
                    self.rfile.readline(); break
                b += self.rfile.read(n); self.rfile.readline()
        else:
            b = self.rfile.read(int(self.headers.get('content-length') or 0))
        if len(sys.argv) > 3: sys.stderr.write(f"{self.command} {self.path} {len(b)}\n")
        if self.headers.get('content-encoding') == 'gzip': b = gzip.decompress(b)
        return b
    def do_HEAD(self): self._send(200, {})
    def do_GET(self):
        p = self.path.split('?')[0]
        if p == '/':
            return self._send(200, {"name": "n", "cluster_name": "c", "version": {"number": "8.19.23", "build_flavor": "default", "lucene_version": "9.12.2", "minimum_wire_compatibility_version": "7.17.0"}, "tagline": "You Know, for Search"})
        return self._send(200, {})
    def do_PUT(self):
        if self.path.split('?')[0].endswith('/_bulk'): return self.do_POST()
        self._body(); self._send(200, {"acknowledged": True})
    def do_POST(self):
        b = self._body()
        p = self.path.split('?')[0]
        if p.endswith('/_bulk'):
            out.write(b if b.endswith(b'\n') else b + b'\n'); out.flush()
            lines = [l for l in b.split(b'\n') if l.strip()]
            items = []
            for l in lines:
                try: o = json.loads(l)
                except Exception: continue
                if len(o) == 1 and list(o)[0] in ('index', 'create', 'update', 'delete') and isinstance(list(o.values())[0], dict):
                    items.append({list(o)[0]: {"_index": "dst", "_id": "x", "status": 201, "result": "created"}})
            return self._send(200, {"took": 0, "errors": False, "items": items})
        if p.endswith('/_refresh'): return self._send(200, {})
        return self._send(200, {})
http.server.ThreadingHTTPServer(('127.0.0.1', int(sys.argv[1])), H).serve_forever()
