# Secret exposure on the host — 2026-11-02, Grok Bot

Target: a local python listener on 127.0.0.1 that accepts and stalls/answers (no real cluster). Binary:
`target/release/elasticdump-rs` at 37fa3ac. Dummy passwords only (`SECRETflag123`, `SECRETurl456`, `SECRETenv789`, `SECRETfile000`).

## Credential inputs the tool supports
`--input-password/--output-password` (alias `--password`), `user:pass@` in the URL, env `ELASTICDUMP_INPUT_USERNAME/PASSWORD`
(and OUTPUT), `--httpAuthFile` (INI `user=`/`password=`), TLS key passphrase `--pass/--input-pass/...`.

## Method
For each input style: start a copy against the listener, then while running read `/proc/<pid>/cmdline`,
`/proc/<pid>/environ`, `ps -eo args`; afterwards grep stderr logs, `/tmp` and `/var/tmp` for the dummy secrets; read
`ulimit -c` and `/proc/sys/kernel/core_pattern`; capture the raw HTTP request.

## Results
| Method | /proc cmdline | ps (any local user) | /proc environ (same uid/root) | stderr | temp files |
|---|---|---|---|---|---|
| `--input-password` flag | **exposed** | **exposed** | no | no | no |
| `user:pass@` in URL | **exposed** | **exposed** | no | no (masked) | no |
| `ELASTICDUMP_INPUT_PASSWORD` env | no | no | exposed (expected; same-uid only) | no | no |
| `--httpAuthFile` (0600) | no | no | no | no | no |

- Error/retry logs never contained the passwords (also covered by `tests/secrets.rs`: `secret_canaries_never_leak`,
  `credential_flags_pick_their_endpoint_and_never_leak`, `debug_log_masks_params_and_credentials`; world-readable auth
  file is warned about: `world_readable_auth_file_is_warned_about`).
- Over plain `http://` the password is on the wire as `authorization: Basic base64(user:pass)` — inherent to Basic auth; use https.
- Core dumps: `ulimit -c` = 0 on this box (core_pattern `/tmp/core.%e.%p.%t`), so no dumps; on hosts with cores
  enabled a crash would write memory (with credentials) to /tmp. The binary does not set `PR_SET_DUMPABLE=0` or `RLIMIT_CORE=0`.

**Verdict:** the env-var and auth-file options already exist and are safe; the flag and URL forms expose the password
to every local user via argv, as with Node elasticdump. **Issues: 1 (medium, documented behaviour).**

## Mitigations / follow-ups
1. Document in README/`--help`: prefer `ELASTICDUMP_*_PASSWORD` or `--httpAuthFile` (0600) over flags/URLs on shared hosts.
2. Print a one-line stderr warning when a password arrives via argv or URL (feature, FEATURES-TODO).
3. Optional: `--input-password-file`/`--password-stdin`, and `prctl(PR_SET_DUMPABLE,0)` + `RLIMIT_CORE=0` at start-up
   (FEATURES-TODO). Argv scrubbing (overwriting `/proc/self/cmdline`) is racy and not recommended.
