# CR/LF header and request injection — 2026-11-02, Grok Bot

Target: raw-socket python listener on 127.0.0.1:19202 capturing every request byte. Binary: release build at 37fa3ac.

## Payloads and results
| Input | Payload | Wire result | Verdict |
|---|---|---|---|
| `--headers` / `--input-headers` / `--output-headers` | CR/LF/NUL/control in name or value | usage error (exit 2), nothing sent, value not echoed — existing test `tests/injection.rs::guards_header_injection_is_a_usage_error` (TST-INJ-011) | safe |
| `--params` value | `b\r\nX-Evil: 1` | `&a=b%0D%0AX-Evil%3A+1` (percent-encoded) | safe |
| `--input-params` key | `a\r\nX-Evil: 2` | `&a%0D%0AX-Evil%3A+2=b` | safe |
| `--input-username` | `u\r\nX-Evil: 3` | inside `authorization: Basic` base64 | safe (no injection) |
| `--input-password` | `p\r\nX-Evil: 4` | inside base64 | safe |
| `--httpAuthFile` password | `p\r\nX-Evil: 5` | trailing CR/LF trimmed (`u:p`) | safe |
| `--input` URL | `.../idx\r\nX-Evil: 6` | URL parser strips CR/LF: path `/idxX-Evil:%206/_search` | no injection; odd index name sent |

No `X-Evil` header and no extra request line appeared in any of the 6 captured requests.

**Gap found and closed:** no regression test covered `--params` CR/LF. Added `tests/injection.rs::guards_params_crlf_is_encoded_not_injected`
(TST-INJ-012) plus a `MockEs::recorded()` helper in `tests/support/mock_es.rs`; `cargo test --test injection guards_` passes (3/3). No code fix needed.

## Follow-ups
- Credentials containing CR/LF are accepted and base64-encoded; consider rejecting control characters in usernames/passwords as a usage error (clearer, matches the header rule).
- An index name from a URL that contained CR/LF becomes `idxX-Evil: 6`; index-name validation could reject `:`/space in URL-derived names before sending (server rejects anyway).
