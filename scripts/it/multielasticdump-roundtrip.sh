#!/usr/bin/env bash
# searchdump-multi dump -> load round trip against two in-memory fake nodes
# (scripts/it/fake_es.py). No containers, no network beyond 127.0.0.1.
# Usage: scripts/it/multielasticdump-roundtrip.sh [BIN_DIR]   (default target/debug; builds it)
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN=${1:-target/debug}
[[ $# -ge 1 ]] || cargo build --quiet --bin searchdump --bin searchdump-multi
M=$BIN/searchdump-multi
W=${KEEP_W:-$(mktemp -d)}; pids=()
trap 'kill "${pids[@]}" 2>/dev/null || true; [[ -n ${KEEP_W:-} ]] || rm -rf "$W"' EXIT
free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
SRC=$(free_port); DST=$(free_port); OSP=$(free_port); DS2=$(free_port)
python3 scripts/it/fake_es.py "$SRC" seed 2>"$W/src.log" & pids+=($!)
python3 scripts/it/fake_es.py "$DST" 2>"$W/dst.log" & pids+=($!)
python3 scripts/it/fake_es.py "$OSP" os 2>"$W/os.log" & pids+=($!)
python3 scripts/it/fake_es.py "$DS2" 2>"$W/ds2.log" & pids+=($!)
for p in $SRC $DST $OSP $DS2; do for _ in $(seq 50); do curl -sf "http://127.0.0.1:$p/" >/dev/null && break; sleep 0.1; done; done
fail=0
ok()  { echo "ok    $*"; }
bad() { echo "FAIL  $*"; fail=1; }
check() { local d=$1; shift; if "$@" >"$W/last.out" 2>&1; then ok "$d"; else bad "$d"; sed 's/^/      /' "$W/last.out"; fi; }
count() { curl -sf "http://127.0.0.1:$1/$2/_count" | python3 -c 'import json,sys;print(json.load(sys.stdin)["count"])'; }
idx() { curl -sf "http://127.0.0.1:$1/$2"; }

# 1. regex + group break-out, zstd3, alias
check "dump group layout (zstd3, alias)" "$M" --input="http://127.0.0.1:$SRC" --output="$W/g" \
  --match='^logs-(?P<group>[a-z]+)-.*$' --layout=group --types=settings,mapping,alias,data --compression=zstd3 --logDir="$W/l1"
check "group dirs app/ and web/" test -s "$W/g/app/logs-app-2026.10.01.data.json.zst" -a -s "$W/g/web/logs-web-2026.10.01.data.json.zst"
check "metrics-1 and .kibana_1 not selected" test ! -e "$W/g/metrics-1.settings.json" -a ! -e "$W/g/app/.kibana_1.settings.json"
check "manifest has 3 done" python3 -c "import json,sys;m=json.load(open('$W/g/multielasticdump.json'));sys.exit(0 if sum(e['status']=='done' for e in m['indices'].values())==3 else 1)"
# 2. load it back with a suffix, lifecycle dropped
check "load with --suffix -r --lifecycle drop" "$M" --direction=load --input="$W/g" --output="http://127.0.0.1:$DST" --suffix=-r --logDir="$W/l2"
check "doc counts 7/3/5" test "$(count $DST logs-app-2026.10.01-r)/$(count $DST logs-app-2026.10.02-r)/$(count $DST logs-web-2026.10.01-r)" = 7/3/5
check "alias restored" sh -c "curl -sf http://127.0.0.1:$DST/logs-app-2026.10.01-r | grep -q '\"logs-app\"'"
check "index.lifecycle dropped" sh -c "! curl -sf http://127.0.0.1:$DST/logs-app-2026.10.01-r | grep -q lifecycle"
check "mapping restored" sh -c "curl -sf http://127.0.0.1:$DST/logs-app-2026.10.01-r/_mapping | grep -q '\"msg\"'"
# 3. existing target: refuse, then --skipExisting
check "reload refuses existing (exit 1)" sh -c "! '$M' --direction=load --input='$W/g' --output=http://127.0.0.1:$DST --suffix=-r --logDir='$W/l3'"
check "reload --skipExisting exits 0" "$M" --direction=load --input="$W/g" --output="http://127.0.0.1:$DST" --suffix=-r --skipExisting --logDir="$W/l4"
# 4. flat gzip, hidden, exclude, empty index; oslogs.py-compatible names
check "dump flat gzip --includeHidden --exclude" "$M" --input="http://127.0.0.1:$SRC" --output="$W/f" --includeHidden --exclude='^metrics' --logDir="$W/l5"
check "oslogs.py file names" test -s "$W/f/.kibana_1.settings.json" -a -s "$W/f/logs-app-2026.10.02.mapping.json" -a -s "$W/f/logs-app-2026.10.02.data.json.gz"
check "load empty and hidden index" "$M" --direction=load --input="$W/f" --output="http://127.0.0.1:$DST" --match='^(\.kibana|empty)' --includeHidden --logDir="$W/l6"
check "empty index created, .kibana_1 has 1 doc" test "$(count $DST empty-logs-x)/$(count $DST .kibana_1)" = 0/1
# 5. load without a manifest (plain oslogs.py directory)
rm "$W/f/multielasticdump.json"
check "load without manifest" "$M" --direction=load --input="$W/f" --output="http://127.0.0.1:$DST" --match='^logs-web' --prefix=nm- --logDir="$W/l7"
check "no-manifest count 5" test "$(count $DST nm-logs-web-2026.10.01)" = 5
# 5b. Node multielasticdump flag names: --ignoreData, --ignoreAlias=false, --includeType, --ignoreChildError
check "dump with --ignoreData --ignoreAlias=false --ignoreChildError" "$M" --input="http://127.0.0.1:$SRC" --output="$W/ni" --match='^logs-app' --ignoreData --ignoreAlias=false --ignoreChildError --logDir="$W/l5b"
check "no data files, alias files written" sh -c "! ls '$W/ni' | grep -q '\.data\.' && test -s '$W/ni/logs-app-2026.10.01.alias.json'"
check "load --includeType=index (settings+mapping only)" "$M" --direction=load --input="$W/f" --output="http://127.0.0.1:$DST" --match='^logs-web' --prefix=it- --includeType=index --ignoreChildError=true --logDir="$W/l5c"
check "index created with mapping and 0 docs" sh -c "test \"\$(curl -sf http://127.0.0.1:$DST/it-logs-web-2026.10.01/_count | python3 -c 'import json,sys;print(json.load(sys.stdin)[\"count\"])')\" = 0 && curl -sf http://127.0.0.1:$DST/it-logs-web-2026.10.01/_mapping | grep -q msg"
# 6. OpenSearch target, drop lifecycle (the default)
check "load into OpenSearch-flavoured node" "$M" --direction=load --input="$W/g" --output="http://127.0.0.1:$OSP" --match='app-2026.10.02' --prefix=os- --logDir="$W/l8"
check "dry run lists, writes nothing" sh -c "'$M' --input=http://127.0.0.1:$SRC --output='$W/dry' --dryRun | grep -q logs-web && test ! -e '$W/dry/multielasticdump.json'"
# 7. alias matching, analyzer, cluster-wide types, split parts
CL=policy,component_template,index_template,template,script
check "dump --matchType alias,index + cluster types + --maxRows 1" "$M" --input="http://127.0.0.1:$SRC" --output="$W/a" \
  --matchType=alias,index --match='^grp-|^logs-web' --types=settings,analyzer,mapping,data,$CL --maxRows=1 --logDir="$W/l9"
check "cluster files written" test -s "$W/a/_cluster/index_template.json" -a -s "$W/a/_cluster/component_template.json" -a -s "$W/a/_cluster/policy.json" -a -s "$W/a/_cluster/template.json" -a -s "$W/a/_cluster/script.json"
check "alias grp-metrics selected metrics-1, split into 2 parts" test -s "$W/a/metrics-1.data.split-0.json.gz" -a -s "$W/a/metrics-1.data.split-1.json.gz" -a ! -e "$W/a/metrics-1.data.split-2.json.gz"
check "manifest records parts and cluster items" python3 -c "import json,sys;m=json.load(open('$W/a/multielasticdump.json'));e=m['indices']['metrics-1'];sys.exit(0 if len(e['data_parts'])==2 and e['records']==2 and len(m['cluster'])==5 and 'logs-app-2026.10.01' not in m['indices'] else 1)"
check "analyzer file has the analysis" grep -q lowercase "$W/a/logs-web-2026.10.01.analyzer.json"
check "--resume skips split dumps" sh -c "'$M' --input=http://127.0.0.1:$SRC --output='$W/a' --matchType=alias --match='^grp-' --types=settings,mapping,data --maxRows=1 --resume --logDir='$W/l10' | grep -q 'skip metrics-1'"
check "load cluster types first, then split parts" sh -c "'$M' --direction=load --input='$W/a' --output=http://127.0.0.1:$DS2 --logDir='$W/l11' >'$W/l11.out' && grep -q 'metrics-1 docs=2 from 2 parts' '$W/l11.out' && grep -q 'logs-web-2026.10.01 docs=5 from 5 parts' '$W/l11.out' && test \"\$(grep -n _cluster/script '$W/l11.out' | cut -d: -f1)\" -lt \"\$(grep -n 'docs=' '$W/l11.out' | head -1 | cut -d: -f1)\""
check "templates, policy, script restored" sh -c "curl -sf http://127.0.0.1:$DS2/_index_template/web-tpl | grep -q composed_of && curl -sf http://127.0.0.1:$DS2/_component_template/base | grep -q timestamp && curl -sf http://127.0.0.1:$DS2/_template/old-tpl | grep -q old- && curl -sf http://127.0.0.1:$DS2/_ilm/policy/logs-policy | grep -q rollover && curl -sf http://127.0.0.1:$DS2/_scripts/add-one | grep -q painless"
check "index template lifecycle dropped" sh -c "! curl -sf http://127.0.0.1:$DS2/_index_template/web-tpl | grep -q lifecycle"
check "analyzer alone creates the index with its analysis" sh -c "'$M' --direction=load --input='$W/a' --output=http://127.0.0.1:$DS2 --match='^logs-web' --types=analyzer,mapping,data --prefix=an- --logDir='$W/l12' && curl -sf http://127.0.0.1:$DS2/an-logs-web-2026.10.01/_settings | grep -q lowercase && test \"\$(curl -sf http://127.0.0.1:$DS2/an-logs-web-2026.10.01/_count | python3 -c 'import json,sys;print(json.load(sys.stdin)[\"count\"])')\" = 5"
check "ILM policy converted to ISM on OpenSearch" sh -c "'$M' --direction=load --input='$W/a' --output=http://127.0.0.1:$OSP --types=policy --lifecycle=convert --logDir='$W/l13' && curl -sf http://127.0.0.1:$OSP/_plugins/_ism/policies/logs-policy | grep -q states"
# 8. data streams
check "dump --matchType datastream" "$M" --input="http://127.0.0.1:$SRC" --output="$W/ds" --matchType=datastream --match='^web$' --types=data,component_template,index_template --logDir="$W/l14"
check "stream files: marker + data, no backing-index files" sh -c "test -s '$W/ds/web.datastream.json' -a -s '$W/ds/web.data.json.gz' && ! ls '$W/ds' | grep -q '^\.ds-'"
check "load data stream into an empty node (create, via its template)" sh -c "'$M' --direction=load --input='$W/ds' --output=http://127.0.0.1:$DST --logDir='$W/l15' | grep -q 'web docs=4'"
check "target has data stream web" sh -c "curl -sf http://127.0.0.1:$DST/_data_stream/web | grep -q '\"name\":\"web\"'"
check "reloading an existing stream is refused" sh -c "! '$M' --direction=load --input='$W/ds' --output=http://127.0.0.1:$DST --logDir='$W/l16'"
# 9. --interval spaces out job starts
check "--interval 400 with 3 indices takes >= 0.8 s" sh -c "s=\$(date +%s%N); '$M' --input=http://127.0.0.1:$SRC --output='$W/iv' --match='^logs-' --types=settings --parallel=3 --interval=400 --logDir='$W/l17' >/dev/null && e=\$(( (\$(date +%s%N)-s)/1000000 )); test \$e -ge 800"
check "--order desc dry run lists newest name first" sh -c "'$M' --input=http://127.0.0.1:$SRC --output='$W/dry' --match='^logs-app' --order=desc --dryRun | grep -E '^  logs-' | head -1 | grep -q 2026.10.02"
check "--fileSize and --maxRows together are refused" sh -c "! '$M' --input=x --output=y --fileSize=1mb --maxRows=2 2>/dev/null"
# 10. --nodeCompat: Node multielasticdump names, metadata outside the directory
check "dump --nodeCompat" "$M" --input="http://127.0.0.1:$SRC" --output="$W/nc" --nodeCompat --match='^logs-' --types=settings,mapping,template,data,policy --logDir="$W/l18"
check "Node names: INDEX.json.gz + gzipped settings/mapping/template" sh -c "for i in logs-app-2026.10.01 logs-web-2026.10.01; do for s in json.gz settings.json.gz mapping.json.gz template.json.gz; do test -s '$W/nc/'\$i.\$s || exit 1; done; done; gzip -t '$W/nc/logs-web-2026.10.01.settings.json.gz'"
check "only index files in the directory; manifest and _cluster in DIR.meta" sh -c "test -z \"\$(ls '$W/nc' | grep -vE '^logs-[a-z]+-2026\.10\.0[12]\.(settings\.|mapping\.|template\.)?json\.gz\$')\" -a -s '$W/nc.meta/multielasticdump.json' -a -s '$W/nc.meta/_cluster/policy.json'"
check "--nodeCompat refuses split, zstd, group layout and data streams" sh -c "! '$M' --input=x --output=y --nodeCompat --maxRows=1 2>/dev/null && ! '$M' --input=x --output=y --nodeCompat --compression=zstd3 2>/dev/null && ! '$M' --input=x --output=y --nodeCompat --layout=index 2>/dev/null && ! '$M' --input=x --output=y --nodeCompat --matchType=datastream 2>/dev/null"
check "our load reads the Node layout (finds DIR.meta)" sh -c "'$M' --direction=load --input='$W/nc' --output=http://127.0.0.1:$DS2 --prefix=nc- --logDir='$W/l19' | grep -q 'nc-logs-web-2026.10.01 docs=5'"
if [[ -n ${NODE_ED_BIN:-} && -x $NODE_ED_BIN/multielasticdump ]]; then
  check "Node multielasticdump loads the --nodeCompat dump" "$NODE_ED_BIN/multielasticdump" --direction=load --input="$W/nc" --output="http://127.0.0.1:$OSP" --fsCompress --interval=50
  check "Node restore: counts 7/3/5" test "$(count $OSP logs-app-2026.10.01)/$(count $OSP logs-app-2026.10.02)/$(count $OSP logs-web-2026.10.01)" = 7/3/5
  check "Node restore: mapping" sh -c "curl -sf http://127.0.0.1:$OSP/logs-web-2026.10.01/_mapping | grep -q '\"msg\"'"
  mkdir -p "$W/nd"
  check "Node multielasticdump dump" "$NODE_ED_BIN/multielasticdump" --direction=dump --input="http://127.0.0.1:$SRC" --output="$W/nd" --match='^logs-web' --fsCompress --interval=50
  check "our load reads Node's own dump" sh -c "'$M' --direction=load --input='$W/nd' --output=http://127.0.0.1:$DS2 --prefix=nd- --logDir='$W/l20' | grep -q 'nd-logs-web-2026.10.01 docs=5'"
  check "our metadata files are byte-identical to Node's" bash -c "for s in settings mapping template; do cmp <(zcat '$W/nc/logs-web-2026.10.01.'\$s.json.gz) <(zcat '$W/nd/logs-web-2026.10.01.'\$s.json.gz) || exit 1; done"
else
  echo "skip  Node checks (set NODE_ED_BIN to the bin dir of an elasticdump 6.x npm install)"
fi
if grep -h UNHANDLED "$W"/*.log; then bad "fake node saw requests it does not handle"; fi
[[ $fail == 0 ]] && echo "searchdump-multi round trip: all checks passed" || { echo "searchdump-multi round trip: FAILED"; exit 1; }
