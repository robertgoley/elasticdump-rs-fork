# Never-executed functions: per-release review (TST-SUP-008)

To regenerate, run `scripts/sup/coverage.sh`. It runs T0 unit, T1 contract and T2 it (es717-plain and opensearch-secure) under cargo-llvm-cov, then calls `scripts/sup/uncovered.py`. Review every named function below at each release. Remove anything nobody calls; anything else needs a reason.

## Review 2026-10-29 (L-205, rewrite after ab01aed)

**Coverage:** functions 91.04% (976/1072), lines 92.16%, regions 84.60%.

**Removed as dead code.** These had no callers anywhere. Because they were `pub` in the lib crate, `dead_code` never fired on them:

- `EsClient::base`
- `EsClient::set_info` (`cfg(test)`, unused)
- `RequestGate::is_noop`
- `OutputTarget::is_stdout` (`FileSink::is_stdout` is the one in use)
- `Sink::report`
- `IndexRenamer::has_rules`
- `impl Clone for IndexRenamer`

**Kept, with a reason:**

| Function | Why it never ran | Decision |
|---|---|---|
| `<SendError as Error>::source` | It is called only when something walks the error chain. anyhow's `{:#}` uses `Display` | Keep: a trait contract, so wrappers keep their cause |
| `<EsClient as Debug>::fmt` | Debug output of the client is never printed. It redacts credentials | Keep: guards against a future `{:?}` leaking secrets. Test candidate for INJ/secrets |
| `NoVerification::verify_tls12_signature` | All test servers negotiate TLS 1.3. The TLS 1.3 path ran | Keep: needed for TLS 1.2-only clusters. Test candidate: a TLS 1.2-only mock (A4) |
| `worker_thread_panic` | Runs only if a worker thread panics | Keep: test candidate for the chaos tier (an injected panic should surface as an error with the worker id) |

The 90 closures that never ran are almost all `.with_context(|| …)`, `map_err` and clap-derive error closures on error paths. They are listed per file below. Error-path coverage is the job of API-050…056 and the chaos tier.

### Closures never run (90)

These are almost all `.with_context(|| …)`/`map_err` error-message closures and the clap derive's error closures. They run only on the matching error path.

| File | Closures |
|---|---|
| `src/cli.rs` | 21 |
| `src/endpoint.rs` | 4 |
| `src/http/client.rs` | 1 |
| `src/http/info.rs` | 1 |
| `src/http/tls.rs` | 4 |
| `src/lib.rs` | 8 |
| `src/meta/alias.rs` | 3 |
| `src/meta/codec.rs` | 2 |
| `src/meta/http.rs` | 2 |
| `src/meta/index_scoped.rs` | 4 |
| `src/meta/mod.rs` | 2 |
| `src/meta/policy/mod.rs` | 3 |
| `src/meta/scripts.rs` | 1 |
| `src/meta/templates.rs` | 4 |
| `src/output.rs` | 10 |
| `src/pipeline/bulk.rs` | 2 |
| `src/pipeline/file_source.rs` | 1 |
| `src/rename.rs` | 2 |
| `src/retrieval/extract.rs` | 2 |
| `src/retrieval/mod.rs` | 4 |
| `src/retrieval/pit.rs` | 1 |
| `src/retrieval/retrieval_task.rs` | 2 |
| `src/retrieval/scroll.rs` | 1 |
| `src/retrieval/search_after.rs` | 1 |
| `src/retrieval/search_body.rs` | 1 |
| `src/retrieval/worker.rs` | 3 |
