"""Cluster-free regression checks for load seeding (L-808l)."""
import importlib.util
import io
from pathlib import Path
import unittest
from unittest.mock import Mock, patch
import urllib.error

spec = importlib.util.spec_from_file_location("load_runner", Path(__file__).with_name("run.py"))
R = importlib.util.module_from_spec(spec)
spec.loader.exec_module(R)


def pair(i):
    return [f'{{"index":{{"_id":"{i}"}}}}\n'.encode(), b'{"message":"seed"}\n']


def reply(*statuses):
    return {"items": [{"index": {"status": status}} for status in statuses]}


class SeedTests(unittest.TestCase):
    def test_verification_defaults_only_reduce_l0(self):
        self.assertEqual(R.verification_limits("L0"), dict(size=100, slices=1))
        for tier in ["L1S", "L1L", "L1", "L2"]:
            self.assertEqual(R.verification_limits(tier), dict(size=1000, slices=4))

    def test_verification_limits_are_configurable(self):
        self.assertEqual(R.verification_limits("L0", 250, 2), dict(size=250, slices=2))
        self.assertEqual(R.verification_limits("L1", slices=1), dict(size=1000, slices=1))
        self.assertEqual(R.positive_int("5"), 5)
        for value in ["0", "-1"]:
            with self.assertRaises(R.argparse.ArgumentTypeError):
                R.positive_int(value)

    def test_request_status_is_preserved(self):
        error = urllib.error.HTTPError("http://localhost/_bulk", 429, "busy", {}, io.BytesIO(b"breaker"))
        with patch.object(R.urllib.request, "urlopen", side_effect=error):
            with self.assertRaises(R.RequestError) as cm:
                R.Cluster("http://localhost").req("POST", "/_bulk", b"x", retry_429=False)
        self.assertEqual(cm.exception.status, 429)
        self.assertIn("HTTP 429", str(cm.exception))

    @patch.object(R.time, "sleep")
    def test_control_request_429_is_retried(self, sleep):
        cl = R.Cluster("http://localhost")
        with patch.object(cl, "_req_once", side_effect=[
                R.RequestError("POST", "/index/_refresh", 429, b"breaker"), {}]) as req:
            self.assertEqual(cl.req("POST", "/index/_refresh"), {})
            self.assertEqual(req.call_count, 2)
        sleep.assert_called_once_with(0.25)

    @patch.object(R.time, "sleep")
    def test_control_request_retry_is_bounded_and_429_only(self, sleep):
        cl = R.Cluster("http://localhost")
        for status, attempts in [(429, 7), (401, 1), (500, 1)]:
            with patch.object(cl, "_req_once", side_effect=R.RequestError("GET", "/", status, b"error")) as req:
                with self.assertRaises(R.RequestError):
                    cl.req("GET", "/")
                self.assertEqual(req.call_count, attempts)

    @patch.object(R.time, "sleep")
    def test_request_429_retries_same_bulk(self, sleep):
        cl = Mock()
        cl.req.side_effect = [R.RequestError("POST", "/_bulk", 429, b"breaker"),
                              R.RequestError("POST", "/_bulk", 429, b"busy"), reply(201)]
        self.assertEqual(R.seed_bulk(cl, pair(1)), [{"status": 201}])
        self.assertEqual([c.args[0] for c in sleep.call_args_list], [0.25, 0.5])
        self.assertEqual([c.args[2] for c in cl.req.call_args_list], [b"".join(pair(1))] * 3)

    @patch.object(R.time, "sleep")
    def test_item_429_does_not_resend_success_or_terminal_rejection(self, sleep):
        cl = Mock()
        cl.req.side_effect = [reply(201, 429, 400), reply(201)]
        result = R.seed_bulk(cl, pair(1) + pair(2) + pair(3))
        self.assertEqual([r["status"] for r in result], [201, 400, 201])
        self.assertEqual(cl.req.call_args_list[1].args[2], b"".join(pair(2)))
        sleep.assert_called_once_with(0.25)

    @patch.object(R.time, "sleep")
    def test_item_then_request_429_keeps_only_unfinished_pairs(self, sleep):
        cl = Mock()
        cl.req.side_effect = [reply(201, 429), R.RequestError("POST", "/_bulk", 429, b"breaker"), reply(201)]
        self.assertEqual(len(R.seed_bulk(cl, pair(1) + pair(2))), 2)
        for c in cl.req.call_args_list[1:]:
            self.assertEqual(c.args[2], b"".join(pair(2)))

    @patch.object(R.time, "sleep")
    def test_persistent_item_429_fails_not_counted_as_bad_document(self, sleep):
        cl = Mock()
        cl.req.return_value = reply(429)
        with self.assertRaisesRegex(RuntimeError, "after 7 attempts"):
            R.seed_bulk(cl, pair(1))
        self.assertEqual(cl.req.call_count, 7)
        self.assertEqual([c.args[0] for c in sleep.call_args_list], [0.25, 0.5, 1.0, 2.0, 4.0, 4.0])

    @patch.object(R.time, "sleep")
    def test_persistent_request_429_is_bounded(self, sleep):
        cl = Mock()
        cl.req.side_effect = R.RequestError("POST", "/_bulk", 429, b"breaker")
        with self.assertRaisesRegex(RuntimeError, "after 7 attempts"):
            R.seed_bulk(cl, pair(1))
        self.assertEqual(cl.req.call_count, 7)

    def test_request_400_isolated_pair_by_pair(self):
        cl = Mock()
        bad = R.RequestError("POST", "/_bulk", 400, b"invalid id")
        cl.req.side_effect = [bad, reply(201), bad]
        result = R.seed_bulk(cl, pair(1) + pair(2))
        self.assertEqual(result, [{"status": 201}, {"status": 400, "error": {"type": "request_400"}}])
        self.assertEqual([c.args[2] for c in cl.req.call_args_list[1:]],
                         [b"".join(pair(1)), b"".join(pair(2))])

    def test_other_request_error_is_not_retried(self):
        cl = Mock()
        cl.req.side_effect = R.RequestError("POST", "/_bulk", 500, b"error")
        with self.assertRaises(R.RequestError):
            R.seed_bulk(cl, pair(1))
        self.assertEqual(cl.req.call_count, 1)

    def test_response_cardinality_mismatch_is_an_error(self):
        cl = Mock()
        cl.req.return_value = reply(201)
        with self.assertRaisesRegex(RuntimeError, "item count"):
            R.seed_bulk(cl, pair(1) + pair(2))

    def test_batch_doc_limit_and_pair_order(self):
        lines = sum((pair(i) for i in range(7)), [])
        batches = list(R.seed_batches(iter(lines), 2, 1 << 20))
        self.assertEqual([len(b) // 2 for b in batches], [2, 2, 2, 1])
        self.assertEqual(sum(batches, []), lines)

    def test_batch_bytes_and_oversized_document_alone(self):
        big = [b'{"index":{}}\n', b'"' + b'x' * 200 + b'"\n']
        lines = pair(1) + big + pair(2) + pair(3)
        cap = 2 * sum(map(len, pair(1)))
        batches = list(R.seed_batches(iter(lines), 500, cap))
        self.assertEqual(sum(batches, []), lines)
        self.assertEqual(batches[1], big)
        for b in batches:
            self.assertTrue(sum(map(len, b)) <= cap or len(b) == 2)

    @patch.object(R.time, "sleep")
    def test_duplicate_id_preserves_last_wins_with_item_retry(self, sleep):
        old = [pair(1)[0], b'{"version":"old"}\n']
        new = [pair(1)[0], b'{"version":"new"}\n']
        batches = list(R.seed_batches(iter(old + pair(2) + new), 500, 1 << 20))
        self.assertEqual(batches, [old + pair(2), new])
        cl = Mock()
        cl.req.side_effect = [reply(429, 201), reply(201), reply(201)]
        for batch in batches:
            R.seed_bulk(cl, batch)
        self.assertEqual([c.args[2] for c in cl.req.call_args_list],
                         [b"".join(old + pair(2)), b"".join(old), b"".join(new)])

    def test_bad_batch_limits_or_truncated_generator_fail(self):
        for docs, size in [(0, 1), (1, 0), (-1, 1)]:
            with self.assertRaises(ValueError):
                list(R.seed_batches([], docs, size))
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            list(R.seed_batches([b'{"index":{}}\n'], 500, 1 << 20))

    def test_empty_seed(self):
        self.assertEqual(list(R.seed_batches([], 500, 1 << 20)), [])


if __name__ == "__main__":
    unittest.main()
