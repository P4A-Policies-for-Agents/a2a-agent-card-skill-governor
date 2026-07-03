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

## Identity

Identity comes from the Anypoint `Authentication` injectable only (no raw token parsing).
`AuthenticationData` exposes `principal`, `client_id`, `client_name`, and
`properties: Value`. `read_identity` reads `client_id`, `client_name`, and scopes;
`Identity` carries exactly those (`client_id`, `client_name`, `scopes`).

Scope is read from the operator-configured `scopeClaimKey` (default `scope`) and accepts
a string or an array of strings.

**SLA tier was removed as an audience in v1.0.0.** An earlier design probed `properties`
for an SLA tier (`sla-tier-name`/`sla-tier-id` plus legacy `tier`/`sla_tier`/`slaTier`
fallbacks) to back an `audienceType: tier`. Using an SLA/rate-limit concept as a
disclosure audience conflated two concerns and required an upstream SLA policy to
populate the property. It is gone: `compile_audience` accepts only `any`/`client`/`scope`,
and a lingering `audienceType: tier` falls into the "unknown audienceType ⇒ WARN + drop"
path (soft, no hard failure).

## Surface axis

Both `VisibilityRule` and `SkillRule` carry an optional `surface` string (config.rs,
hand-maintained) compiled by `compile_surface` into a `RuleSurface` (`Any`/`Public`/
`Extended`) stored on `CompiledVis`/`CompiledUpsert`. `RuleSurface::matches(actual)`
gates each rule against the request's detected `Surface` and is ANDed with audience and
target in `govern()`. Omitted / `any` ⇒ both surfaces (backward-compatible); unknown
value ⇒ WARN + `Any`. The axis is orthogonal to audience, so `surface: extended` +
`audienceType: any` expresses "any authenticated caller".

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
- The `pdk-test` harness cannot inject a populated `Authentication` (no auth policy in the
  mock chain), so `client`/`scope`-audience matching is not covered end-to-end by
  integration tests; confirm those against a real deployment behind an auth policy. The
  `surface` axis IS covered end-to-end (`surface_public_rule_*` tests): the same
  `surface: public` ruleset hides a skill on the well-known card and no-ops on the
  extended card, since surface detection needs no identity.

## Local manual testing

See the local-testing skill at
`a2a-agent-card-skill-governor-flex/.claude/test-a2a-agent-card-skill-governor-locally/SKILL.md`
for `make run` + `curl` recipes hitting all three card surfaces. (That path is gitignored,
so it is present in a working checkout but not committed.)
