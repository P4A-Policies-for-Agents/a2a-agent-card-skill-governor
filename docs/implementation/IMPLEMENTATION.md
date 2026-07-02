# Implementation Notes — A2A Agent Card Skill Governor

Companion to [`../architecture/ARCHITECTURE.md`](../architecture/ARCHITECTURE.md). This
document is for engineers editing the code: build/test workflow, module internals,
and the non-obvious constraints that will bite a future editor.

## Build & test

Use `make` — never `cargo` directly (keeps the split-model asset copy in sync).

```bash
# from a2a-agent-card-skill-governor-flex/
make build        # compile to wasm32-wasip1 + copy gcl.yaml into the asset layout
make test-unit    # pure Rust unit tests (no Docker) — 20 tests
make test         # integration tests — requires Docker + a registered Flex Gateway
make run          # start the playground (mock upstream + gateway) for manual curl
```

`config.rs` is **hand-maintained** here — `make build-asset-files` does NOT regenerate
it in this project. Any change to `a2a-agent-card-skill-governor-definition/gcl.yaml`
must be mirrored by hand into `src/generated/config.rs` (struct fields + serde aliases).

## Module map

| File | Role |
|---|---|
| `src/lib.rs` | Entrypoint `configure`; `request_filter` (classify + read identity + thread `CardContext`); `response_filter` (split-flow body read/write); `transform_card_body` (pure card transform, unit-testable); `fail_closed_body`; identity read from `Authentication`. |
| `src/detect.rs` | `classify_surface(method, path, is_v1, jsonrpc_method) -> Option<(Surface, Variant)>`. |
| `src/governor.rs` | `GovernorRules::compile` (config-time validate + WARN) / `govern` (per-response). `Audience::matches`, inline glob matcher. The rule engine core. |
| `src/a2a.rs` | `Skill` (typed governed fields + `#[serde(flatten)] extra` for round-tripping unknown fields), `is_agent_card`, A2A method-name constants. |
| `src/generated/config.rs` | Hand-maintained config structs mirroring `gcl.yaml`. |

## Load-bearing constraints (do not regress)

1. **Response filter must use the split flow** `into_headers_state()` → `into_body_state()`.
   The combined stop-iteration state (`into_headers_body_state()`) **hangs on the
   response leg on Flex 1.12.1** → every response-transforming request returns HTTP
   504. This was found by live integration testing and confirmed against the working
   sibling policies (`rest-to-a2a`, `mcp-apps`), which also use the split flow on the
   response path. The `enable_stop_iteration` Cargo feature is intentionally **absent**.
   Do not reintroduce the combined state to "simplify" header+body access.

2. **content-length is removed in the headers phase**, before the body is read, so the
   gateway recomputes it from the new bytes. Setting it by hand is fragile.

3. **Fail-closed is body-only.** On the split flow, HTTP status is committed in the
   headers phase — before the body (and thus the fail-closed decision) is known — so a
   body-content-dependent 500 is impossible. Fail-closed replaces the card **body** with
   an error envelope and leaves the status as upstream sent it. This still satisfies the
   security property: ungoverned skills are never shipped because the body is replaced.
   `transform_card_body` returns `Err` only *after* `is_agent_card` confirms the body is
   a card; every `Err` path overwrites the body.

4. **No `unwrap`/`panic` on attacker-influenced data** (upstream response body, request
   headers, JSON-RPC envelope). A panic in a wasm filter is a DoS. The only
   `unwrap_or_else` calls are on fixed static fallback bodies in `fail_closed_body`, so
   the fail-closed path itself cannot panic.

## Identity & SLA tier

Identity comes from the Anypoint `Authentication` injectable only (no raw token parsing).
`AuthenticationData` exposes `principal`, `client_id`, `client_name`, and
`properties: Value` — there is **no first-class tier field**. The caller's SLA tier is
read from `properties` under `sla-tier-name` (human name, e.g. `Gold`) / `sla-tier-id`,
which the SLA-based rate-limiting policy (`rate_limit_sla`) propagates; legacy spellings
(`tier`/`sla_tier`/`slaTier`) are probed as fallbacks. `tier`-audience rules therefore
require an upstream SLA-tier policy in the chain; absent it, tier is `None` and those
rules never match (graceful degrade, no error).

Scope is read from the operator-configured `scopeClaimKey` (default `scope`) and accepts
a string or an array of strings.

## Testing notes

- Unit tests live inline (`#[cfg(test)]` in each module) + `resp_tests` in `lib.rs` for
  the pure `transform_card_body`. Keep `transform_card_body` a pure free function so it
  stays testable without a PDK harness.
- Integration tests (`tests/requests.rs`) are black-box HTTP assertions against a mock
  upstream returning a fixed AgentCard, using the `pdk-test` harness. The policy is bound
  in the **outbound** slot (`.outbound_policies(...)`) — the inbound slot rejects an
  outbound-injection policy.
- `tests/config/registration.yaml` is device-local (gitignored) — another machine must
  supply its own before `make test`.
- The `pdk-test` harness cannot inject a populated `Authentication`/SLA tier (no auth/SLA
  policy in the mock chain), so `tier`-audience matching is not covered end-to-end by
  integration tests; confirm it against a real deployment behind an SLA-tier policy.

## Local manual testing

See the local-testing skill at
`a2a-agent-card-skill-governor-flex/.claude/test-a2a-agent-card-skill-governor-locally/SKILL.md`
for `make run` + `curl` recipes hitting all three card surfaces. (That path is gitignored,
so it is present in a working checkout but not committed.)
