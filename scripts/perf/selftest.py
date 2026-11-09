#!/usr/bin/env python3
"""Offline protocol tests. No podman, engines or Rust binaries required."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import warnings
from unittest.mock import patch

import suite as S


def metric(wall=10):
    return dict(wall_s=wall, docs_per_s=100 / wall, cpu_s=2, user_s=1, sys_s=1,
                max_rss_kb=100, fds_peak=5, mb_per_s_in=10 / wall, mb_per_s_out=5 / wall,
                compression_ratio=2, input_bytes=100, output_bytes=50, exit_code=0, failed_writes=0)


def args(**kw):
    d = dict(tier="L0", place="big", engines=None, cases=None, skip_node=False, reps=5, threshold=10)
    d.update(kw)
    return argparse.Namespace(**d)


class ProtocolTests(unittest.TestCase):
    def test_full_plan_covers_every_engine_image(self):
        plan = S.make_plan(args())
        self.assertEqual(len(plan["engines"]), 16)
        self.assertEqual(len({c['id'] for c in S.CASES}), 37)
        lock = {line.split()[0] for line in (S.ROOT / 'scripts/it/images.lock').read_text().splitlines()
                if line and not line.startswith('#')}
        engines = {e['image'].split('@')[0] for e in plan['engines']}
        helpers = {i for i in lock if '/alpine:' in i or '/toxiproxy:' in i}
        self.assertEqual(engines, lock - helpers)
        self.assertTrue(all('@sha256:' in e['image'] for e in plan['engines']))
        self.assertEqual(S.make_plan(args(tier='L1'))['heap_mb'], 4096)

    def test_opensearch_23_uses_tier_heap(self):
        # After up.sh disables query-insights top_queries, OS 2/3 share the tier heap.
        p = S.make_plan(args(tier='L0', place='box', engines='os2-plain,os3-secure,es717-plain,os13-plain'))
        self.assertEqual(p['heap_mb'], 1024)
        by = {e['engine']: e['heap_mb'] for e in p['engines']}
        self.assertEqual(by['os2-plain'], 1024)
        self.assertEqual(by['os3-secure'], 1024)
        self.assertEqual(by['es717-plain'], 1024)
        self.assertEqual(by['os13-plain'], 1024)
        p1 = S.make_plan(args(tier='L1', place='big', engines='os2-plain'))
        self.assertEqual(p1['engines'][0]['heap_mb'], 4096)

    def test_selection_and_exclusions(self):
        p = S.make_plan(args(engines='os3-secure,es710-plain', cases='copy,dump-pit-s4'))
        self.assertEqual(p['engine_order'], ['es710-plain', 'os3-secure'])
        pit = next(c for c in p['engines'][0]['cases'] if c['id'] == 'dump-pit-s4')
        self.assertNotEqual(pit['tools']['rs'], 'RUN')
        with self.assertRaises(ValueError):
            S.make_plan(args(engines='unknown'))
        with self.assertRaises(ValueError):
            S.make_plan(args(cases='copy,copy'))
        with self.assertRaises(ValueError):
            S.make_plan(args(tier='L1', place='shared'))
        self.assertEqual(S.make_plan(args(skip_node=True))['engines'][0]['cases'][0]['tools'], {'rs': 'RUN'})

    def test_aggregate(self):
        a = S.aggregate([metric(x) for x in (4, 1, 5, 2, 3)])
        self.assertEqual(a['wall_s'], 3)
        self.assertEqual(a['wall_p90_s'], 5)
        self.assertEqual((a['wall_min_s'], a['wall_max_s']), (1, 5))
        with self.assertRaises(ValueError):
            S.aggregate([dict(metric(), exit_code=1)])
        with self.assertRaises(ValueError):
            S.aggregate([])

    def test_compare_and_summary(self):
        with tempfile.TemporaryDirectory() as td:
            p = Path(td)
            a = S.row_summary('copy', {'rs': metric(), 'node': metric(20)}, metric(), 10, p)
            self.assertEqual(a['status'], 'PASS')
            self.assertEqual(a['rs_over_node'], .5)
            a = S.row_summary('copy', {'rs': metric(12), 'node': metric()}, metric(), 10, p)
            self.assertEqual(a['status'], 'REGRESSION')
            self.assertAlmostEqual(a['baseline_delta_pct'], 20)
            # Node being quicker does NOT fail a regression-free Rust result.
            a = S.row_summary('copy', {'rs': metric(), 'node': metric(5)}, metric(), 10, p)
            self.assertEqual(a['status'], 'PASS')
            self.assertEqual(a['node_comparison'], 'SLOWER')
            a = S.row_summary('copy', {'rs': metric()}, None, 10, p)
            self.assertIn('NO_BASELINE', a['status'])
            self.assertIsNone(a['node'])
            a = S.row_summary('copy', {}, metric(), 10, p)
            self.assertEqual(a['status'], 'FAIL')
            self.assertTrue(S.compare_metrics(metric(), dict(metric(), max_rss_kb=120), 10, p))
            self.assertFalse(S.compare_metrics(metric(), metric(11), 10, p))
            text = S.markdown({'engines': [dict(engine='os2-plain', summary=[a, dict(case='pit', status='SKIP')],
                                                failures=['synthetic error'])]})
            self.assertIn('| copy |', text)
            self.assertIn('synthetic error', text)
            self.assertIn('SKIP', text)

    def test_invalid_metrics_fail(self):
        with tempfile.TemporaryDirectory() as td:
            self.assertTrue(S.compare_metrics(metric(), dict(metric(), failed_writes=1), 10, td))
            self.assertTrue(S.compare_metrics(metric(), dict(metric(), exit_code=1), 10, td))

    def test_baseline_identity_no_start(self):
        # Identity mismatches must fail without provisioning or overwriting a baseline.
        with tempfile.TemporaryDirectory(dir=S.ROOT / 'results') as td:
            d = Path(td)
            p = S.make_plan(args(engines='es717-plain', cases='copy'))
            a = args(update_baselines=True, baseline_dir=d)
            host = S.os.environ.get('PERF_HOST_ID', S.platform.node())
            bpath = d / f'{host}-big-L0-es717-plain.json'
            S.write_json(bpath, {'identity': {}, 'cells': {'copy': metric()}})
            original = bpath.read_text()
            out = d / 'engine'
            out.mkdir()
            with patch.object(S.EngineRun, 'start') as start, patch.object(S.EngineRun, 'close'):
                res = S.execute_engine(a, p, p['engines'][0], {}, out)
                start.assert_not_called()
            self.assertTrue(res['failures'])
            self.assertEqual(original, bpath.read_text())

    def test_measure_wrapper_schema(self):
        with tempfile.TemporaryDirectory() as td:
            p = Path(td)
            out = p / 'data.ndjson'
            metrics = p / 'metrics.json'
            subprocess.run([str(S.LOAD / 'measure.sh'), '--out', str(metrics), '--docs', '2', '--output', str(out),
                            '--', sys.executable, '-c', f"from pathlib import Path; Path({str(out)!r}).write_text('{{}}\\n{{}}\\n')"],
                           check=True, capture_output=True)
            m = json.loads(metrics.read_text())
            with warnings.catch_warnings():
                warnings.simplefilter('ignore', ResourceWarning)  # legacy validator opens its schema inline
                self.assertEqual(S.M.validate(m), [])
            self.assertEqual(m['exit_code'], 0)
            self.assertEqual(m['docs'], 2)
            self.assertGreater(m['output_bytes'], 0)

    def test_baseline_merge_and_failed_run_preserves(self):
        with tempfile.TemporaryDirectory(dir=S.ROOT / 'results') as td:
            d = Path(td)
            p = S.make_plan(args(engines='es717-plain', cases='copy'))
            a = args(update_baselines=True, baseline_dir=d, reps=2)
            def invoke(name, failure=False):
                directory = d / name
                directory.mkdir()
                with patch.object(S.EngineRun, 'start'), patch.object(S.EngineRun, 'seed'), \
                     patch.object(S.EngineRun, 'cleanup_dataset'), patch.object(S.EngineRun, 'close'), \
                     patch.object(S.EngineRun, 'iteration', side_effect=RuntimeError('bad digest') if failure else None,
                                  return_value=metric()):
                    return S.execute_engine(a, p, p['engines'][0], {}, directory)
            res = invoke('ok')
            self.assertEqual(res['failures'], [])
            bpath = Path(res['baseline_updated'])
            base = json.loads(bpath.read_text())
            base['cells']['unselected'] = metric()
            S.write_json(bpath, base)
            res = invoke('partial')
            self.assertIn('unselected', json.loads(bpath.read_text())['cells'])
            before = bpath.read_text()
            res = invoke('failed', True)
            self.assertTrue(res['failures'])
            self.assertEqual(res['summary'][0]['status'], 'FAIL')
            self.assertEqual(before, bpath.read_text())

    def test_node_failure_is_recorded_not_an_rs_failure(self):
        with tempfile.TemporaryDirectory(dir=S.ROOT / 'results') as td:
            d = Path(td)
            p = S.make_plan(args(engines='es717-plain', cases='dump-small'))
            a = args(update_baselines=True, baseline_dir=d, reps=2)
            calls = []
            def it(case, tool, rep):
                calls.append((tool, rep))
                if tool == 'node':
                    raise S.VerifyMismatch('document sets differ')
                return metric()
            with patch.object(S.EngineRun, 'start'), patch.object(S.EngineRun, 'seed'), \
                 patch.object(S.EngineRun, 'cleanup_dataset'), patch.object(S.EngineRun, 'close'), \
                 patch.object(S.EngineRun, 'iteration', side_effect=it):
                res = S.execute_engine(a, p, p['engines'][0], {}, d)
            self.assertEqual(res['failures'], [])
            row = res['summary'][0]
            self.assertEqual(row['status'], 'PASS (NO_BASELINE)')
            self.assertEqual(row['node_status'], 'NODE_DIFF')
            self.assertIsNone(row['node'])
            # Node stops after its first failure; rs completes warm-up plus every repetition.
            self.assertEqual([c for c in calls if c[0] == 'node'], [('node', -1)])
            self.assertEqual(len([c for c in calls if c[0] == 'rs']), 3)
            self.assertIn('NODE_DIFF', S.markdown(dict(engines=[res])))
            self.assertIn('baseline_updated', res)

    def test_timeout_kills_process_group(self):
        with tempfile.TemporaryDirectory() as td:
            marker = Path(td) / 'child-alive'
            # The wrapper forks a grandchild (like measure.py -> Node -> forked elasticdump).
            script = (f"import subprocess,sys,time; subprocess.Popen([sys.executable,'-c',"
                      f"\"import time,pathlib; time.sleep(3); pathlib.Path({str(marker)!r}).write_text('x')\"]); time.sleep(60)")
            with self.assertRaises(S.ToolTimeout):
                S.checked([sys.executable, '-c', script], timeout_s=1)
            import time
            time.sleep(3.5)
            self.assertFalse(marker.exists())
        with self.assertRaises(subprocess.CalledProcessError):
            S.checked([sys.executable, '-c', 'raise SystemExit(3)'], timeout_s=30)

    def test_node_timeout_is_recorded_not_an_rs_failure(self):
        with tempfile.TemporaryDirectory(dir=S.ROOT / 'results') as td:
            d = Path(td)
            p = S.make_plan(args(engines='es717-plain', cases='multi-restore-p1'))
            a = args(update_baselines=False, baseline_dir=d, reps=1)
            def it(case, tool, rep):
                if tool == 'node':
                    raise S.ToolTimeout('timed out')
                return metric()
            with patch.object(S.EngineRun, 'start'), patch.object(S.EngineRun, 'seed'), \
                 patch.object(S.EngineRun, 'cleanup_dataset'), patch.object(S.EngineRun, 'close'), \
                 patch.object(S.EngineRun, 'iteration', side_effect=it):
                res = S.execute_engine(a, p, p['engines'][0], {}, d)
            self.assertEqual(res['failures'], [])
            self.assertEqual(res['summary'][0]['node_status'], 'NODE_TIMEOUT')

    def test_node_auth_file_is_ini(self):
        text = S.node_auth_ini('admin', 'p;w#d="x"')
        self.assertEqual(text, 'user="admin"\npassword="p;w#d=\\"x\\""\n')
        node = Path(S.os.environ.get('NODE_ED_BIN', '/workspace/node-ed/node_modules/.bin')).parent / 'elasticdump'
        if (node / 'lib/add-auth.js').exists() and S.shutil.which('node'):
            with tempfile.NamedTemporaryFile('w', suffix='.ini') as f:
                f.write(text)
                f.flush()
                js = (f"console.log(require({str(node / 'lib/add-auth.js')!r})"
                      f"('http://h:1/x', {f.name!r}))")
                out = subprocess.run(['node', '-e', js], capture_output=True, text=True, check=True).stdout
            self.assertEqual(out.strip(), 'http://admin:p%3Bw%23d%3D%22x%22@h:1/x')

    def test_settings_target_is_left_absent(self):
        with tempfile.TemporaryDirectory(dir=S.ROOT / 'results') as td:
            plan = S.make_plan(args(engines='es717-plain', cases='settings-restore'))
            er = S.EngineRun(args(), plan, plan['engines'][0], {}, Path(td))
            er.cl = unittest.mock.Mock()
            er.create_target('perf-src', 'perf-dst', meta='settings')
            self.assertEqual([c.args[0] for c in er.cl.req.call_args_list], ['DELETE'])

    def test_multi_cli_child_flags_and_fresh_targets(self):
        with tempfile.TemporaryDirectory(dir=S.ROOT / 'results') as td:
            directory = Path(td)
            plan = S.make_plan(args(engines='es717-plain', cases='multi-restore-p4'))
            entry = plan['engines'][0]
            er = S.EngineRun(args(), plan, entry, {'multielasticdump': 'multi'}, directory)
            er.url = 'http://127.0.0.1:1'
            er.tenv = {}
            er.rs_tls, er.node_tls = [], []
            er.sources = [dict(index='perf-src-00', stored=100)]
            er.count, er.raw_bytes = 100, 1000
            er.refdir = er.work / 'refs'
            er.refdir.mkdir()
            er.cl = unittest.mock.Mock()
            er.verify = unittest.mock.Mock()
            commands = []
            def measured(command, **_kw):
                commands.append(command)
                out = Path(command[command.index('--out') + 1])
                S.write_json(out, metric())
            with patch.object(S, 'checked', side_effect=measured), patch.object(S.M, 'validate', return_value=[]), \
                 patch.object(S.R, 'leak_check', return_value=({}, [])):
                er.iteration(entry['cases'][0], 'rs', 0)
            command = commands[0]
            child = command[command.index('multi'):]
            self.assertEqual(child[child.index('--') + 1:], ['--limit=2000', '--concurrency=1'])
            self.assertIn('--compression=none', child)
            self.assertIn('--parallel=4', child)
            # Existing target creation would make Rust multi refuse the load.
            self.assertFalse(any(c.args[0] == 'PUT' for c in er.cl.req.call_args_list))
            er.verify.assert_called_once()

    def test_split_wrapper_failure_stops(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / 'ran'
            command = [sys.executable, str(S.HERE / 'parts.py'), '--files', 'one', 'two', '--',
                       sys.executable, '-c', f"from pathlib import Path; Path({str(path)!r}).write_text('one'); raise SystemExit(7)"]
            self.assertEqual(subprocess.run(command).returncode, 7)
            self.assertEqual(path.read_text(), 'one')

    def test_metadata_normalization(self):
        cl = unittest.mock.Mock()
        cl.req.return_value = {'src': {'settings': {'index': {'number_of_shards': '1', 'uuid': 'x', 'version': '9',
                                                            'creation_date': '42', 'provided_name': 'src'}}}}
        self.assertEqual(S.meta_value(cl, 'src', 'settings'), {'number_of_shards': '1'})
        cl.req.return_value = {'src': {'mappings': {'properties': {'x': {'type': 'keyword'}}}}}
        self.assertIn('properties', S.meta_value(cl, 'src', 'mappings'))
        cl.req.assert_called_with('GET', '/src/_mapping')

    def test_down_scopes_container_and_volume_cleanup(self):
        with tempfile.TemporaryDirectory() as td:
            d = Path(td)
            fake = d / 'podman'
            log = d / 'calls'
            fake.write_text("#!/bin/bash\nprintf '%s ' \"$@\" >> \"$TEST_LOG\"; printf '\\n' >> \"$TEST_LOG\"\n"
                            "if [[ \"$1\" == ps ]]; then echo our-container; fi\n"
                            "if [[ \"$1 $2\" == 'volume ls' ]]; then echo our-volume; fi\n")
            fake.chmod(0o755)
            env = dict(S.os.environ, PATH=str(d) + ':' + S.os.environ['PATH'], TEST_LOG=str(log),
                       IT_DIR=str(d / 'private'), RUN_ID='suite-run')
            subprocess.run([str(S.ROOT / 'scripts/it/down.sh')], env=env, check=True)
            calls = log.read_text()
            self.assertIn('name=-suite-run$', calls)
            self.assertIn('label=elasticdump-rs-run=suite-run', calls)
            self.assertIn('rm -f -v our-container', calls)
            self.assertIn('volume rm our-volume', calls)

    def test_offline_dry_run_has_no_results_side_effects(self):
        with patch.object(S, 'prerequisites', side_effect=AssertionError('must not run')):
            p = subprocess.run([str(S.HERE / 'run-suite.sh'), '--dry-run', '--engines', 'os2-plain', '--skip-node'],
                               capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(p.stdout)['engine_order'], ['os2-plain'])


if __name__ == '__main__':
    unittest.main()
