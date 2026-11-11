#!/usr/bin/env python3
"""TST-API-085 / L-506: differential bulk encoding against Node elasticdump.

N seeded random documents (default 10000) are restored file -> "cluster" by Node elasticdump
and by searchdump, each against scripts/node/recorder.py, which records the _bulk NDJSON.
Per document (keyed by _id) the action line and the source line are compared byte for byte.
A difference is "documented" only if it is fully explained by Node re-serialising the parsed
JSON (escape forms, number forms, integer-like key order: README "Intentional differences"),
checked by Node itself: JSON.stringify(JSON.parse(rust_line)) == node_line, or by Node's
big-integer handling (an integer beyond 2^53 sent as a string). Anything else is UNDOCUMENTED
and fails the run.

  scripts/node/bulk-diff.py [--n 10000] [--seed 506] [--node-ed PATH] [--bin PATH] [--json OUT]
"""
import argparse, json, os, random, subprocess, sys, tempfile, time, socket

ap = argparse.ArgumentParser()
ap.add_argument('--n', type=int, default=10000)
ap.add_argument('--seed', type=int, default=506)
ap.add_argument('--node-ed', default=os.environ.get('NODE_ELASTICDUMP', '/tmp/ed/node_modules/elasticdump/bin/elasticdump'))
ap.add_argument('--bin', default='target/release/searchdump')
ap.add_argument('--json')
a = ap.parse_args()
here = os.path.dirname(os.path.abspath(__file__))
R = random.Random(a.seed)

ALPH = ['a', 'Z', '0', ' ', 'é', '中', '😀', '"', '\\', '/', '\n', '\t', '\x01', '\x7f', '\u2028', '\ufeff']
def raw_str(s):
    """JSON string literal with randomly chosen (valid) escape forms."""
    out = ['"']
    for ch in s:
        c = ord(ch)
        r = R.random()
        if ch == '"': out.append('\\"')
        elif ch == '\\': out.append('\\\\')
        elif ch == '/' and r < .5: out.append('\\/')
        elif c < 0x20 or (r < .15 and c < 0x10000):
            out.append({'\n': '\\n', '\t': '\\t'}.get(ch) if r < .5 and ch in '\n\t' else '\\u%04x' % c if r < .8 else '\\u%04X' % c)
        elif c >= 0x10000 and r < .2:
            v = c - 0x10000; out.append('\\u%04x\\u%04x' % (0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF)))
        else: out.append(ch)
    out.append('"'); return ''.join(out)
def rstr(maxlen=8): return ''.join(R.choice(ALPH) for _ in range(R.randint(0, maxlen)))
NUMS = ['0', '-0', '1', '-17', '1.0', '1.50', '3.14159', '1e3', '1E+3', '2.5e-7', '-0.0', '1e21', '9007199254740993',
        '12345678901234567890', '-9223372036854775808', '0.1', '100', '1.7976931348623157e308', '5e-324']
def rnum():
    return R.choice(NUMS) if R.random() < .6 else str(R.randint(-10**6, 10**6)) if R.random() < .5 else repr(R.uniform(-1e6, 1e6))
def rkey():
    return R.choice([rstr(6), str(R.randint(0, 20)), 'k%d' % R.randint(0, 5), '_id', 'a.b'])
def rval(d=0):
    t = R.random()
    if d > 3 or t < .35: return raw_str(rstr())
    if t < .6: return rnum()
    if t < .7: return R.choice(['true', 'false', 'null'])
    if t < .85: return '[' + ','.join(rval(d + 1) for _ in range(R.randint(0, 4))) + ']'
    return robj(d + 1)
def robj(d=0):
    keys, seen = [], set()
    for _ in range(R.randint(0, 5)):
        k = rkey()
        if k not in seen: seen.add(k); keys.append(k)   # duplicate keys: separate concern (INJ-008)
    ws = lambda: R.choice(['', '', ' '])
    return '{' + ','.join(f'{ws()}{raw_str(k)}{ws()}:{ws()}{rval(d)}' for k in keys) + '}'

docs = []
for i in range(a.n):
    meta = [f'"_index":"src"', f'"_id":{raw_str("d%d" % i)}']
    if R.random() < .1: meta.append('"_score":1.0')
    docs.append('{' + ','.join(meta) + f',"_source":{robj()}' + '}')

def free_port():
    s = socket.socket(); s.bind(('127.0.0.1', 0)); p = s.getsockname()[1]; s.close(); return p

def run(tool, inp, rec):
    port = free_port()
    srv = subprocess.Popen([sys.executable, os.path.join(here, 'recorder.py'), str(port), rec])
    time.sleep(0.7)
    try:
        cmd = (['node', a.node_ed] if tool == 'node' else [a.bin]) + [f'--input={inp}', f'--output=http://127.0.0.1:{port}/dst', '--type=data', '--limit=500']
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=900)
        if p.returncode != 0: sys.exit(f'{tool} failed rc={p.returncode}: {p.stderr[-2000:]}')
    finally:
        srv.terminate(); srv.wait()

def pairs(path):
    ls = [l for l in open(path, 'rb').read().split(b'\n') if l]
    out = {}
    for act, src in zip(ls[0::2], ls[1::2]):
        _id = json.loads(act)['index']['_id']
        out[_id] = (act, src)
    return out

with tempfile.TemporaryDirectory() as td:
    inp = os.path.join(td, 'in.ndjson'); open(inp, 'w', encoding='utf-8').write('\n'.join(docs) + '\n')
    rn, rr = os.path.join(td, 'node.bulk'), os.path.join(td, 'rs.bulk')
    open(rn, 'w').close(); open(rr, 'w').close()
    run('node', inp, rn); run('rust', inp, rr)
    N, Rs = pairs(rn), pairs(rr)
    # Node's own re-serialisation of every Rust line, in one process
    keys = sorted(set(N) & set(Rs))
    lines = os.path.join(td, 'rs.lines'); open(lines, 'wb').write(b'\n'.join(l for k in keys for l in Rs[k]) + b'\n')
    js = ("const ls=require('fs').readFileSync(process.argv[1],'utf8').split('\\n');ls.pop();"
          "process.stdout.write(ls.map(l=>JSON.stringify(JSON.parse(l))).join('\\n')+'\\n')")
    reser = subprocess.run(['node', '-e', js, lines], capture_output=True, check=True).stdout.split(b'\n')
    stats = {'docs': a.n, 'node_docs': len(N), 'rust_docs': len(Rs), 'byte_equal': 0,
             'documented_reserialisation': 0, 'documented_bigint': 0, 'undocumented': 0,
             'action_lines_equal': 0, 'node_order_differs': None}
    examples = []
    def bigint_ok(rs_line, node_line):
        # Node elasticdump 6.x turns some integers beyond 2^53 into strings and rounds others
        # (e.g. -9223372036854775808 -> -9223372036854776000); Rust keeps the digits.
        def eq(r, n):
            if isinstance(r, dict):
                return isinstance(n, dict) and r.keys() == n.keys() and all(eq(r[k], n[k]) for k in r)
            if isinstance(r, list):
                return isinstance(n, list) and len(r) == len(n) and all(map(eq, r, n))
            if isinstance(r, int) and not isinstance(r, bool) and abs(r) > 2**53:
                return n == str(r) or (isinstance(n, (int, float)) and not isinstance(n, bool) and float(n) == float(r))
            if isinstance(r, (int, float)) and not isinstance(r, bool):
                return isinstance(n, (int, float)) and not isinstance(n, bool) and float(n) == float(r)
            return r == n
        return eq(json.loads(rs_line), json.loads(node_line))
    for i, k in enumerate(keys):
        (na, ns), (ra, rs) = N[k], Rs[k]
        node_re_a, node_re_s = reser[2 * i], reser[2 * i + 1]
        if na == ra: stats['action_lines_equal'] += 1
        if na == ra and ns == rs: stats['byte_equal'] += 1; continue
        if node_re_a == na and node_re_s == ns: stats['documented_reserialisation'] += 1; continue
        if node_re_a == na and bigint_ok(rs, ns): stats['documented_bigint'] += 1; continue
        stats['undocumented'] += 1
        if len(examples) < 5: examples.append({'id': k, 'node': (na + b'\n' + ns).decode('utf-8', 'replace'), 'rust': (ra + b'\n' + rs).decode('utf-8', 'replace')})
    stats['missing'] = sorted(set(N) ^ set(Rs))[:5]
    on = [json.loads(l)['index']['_id'] for l in open(rn, 'rb').read().split(b'\n')[0::2] if l]
    orr = [json.loads(l)['index']['_id'] for l in open(rr, 'rb').read().split(b'\n')[0::2] if l]
    stats['node_order_differs'] = on != orr
    stats['rust_order_is_input_order'] = orr == ['d%d' % i for i in range(a.n)]
    stats['examples'] = examples
    ok = stats['undocumented'] == 0 and not stats['missing'] and stats['rust_order_is_input_order']
    stats['pass'] = ok
    print(json.dumps(stats, indent=1, ensure_ascii=False))
    if a.json: json.dump(stats, open(a.json, 'w'), indent=1, ensure_ascii=False)
    sys.exit(0 if ok else 1)
