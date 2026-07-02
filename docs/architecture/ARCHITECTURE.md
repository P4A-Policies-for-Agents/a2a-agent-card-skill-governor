# A2A Agent Card Skill Governor — Architecture

> Durable, version-controlled architecture reference for the shipped policy.
> Reflects the implementation as verified: unit 20/20, clean build, live
> integration 12/12 on Flex Gateway 1.12.1. Terminology tracks the code and the
> Flex toolchain ("Flex", not "Omni").

## 1. Overview

The A2A Agent Card Skill Governor is a **Flex Gateway outbound response-shaping
policy** (PDK 1.9.0, Rust → `wasm32-wasip1`, split-model) that governs the
`skills[]` array of an A2A **Agent Card** in-flight, before the card reaches the
calling client. It never blocks a request; it reshapes the response body. Two
governance actions combine per **Model C**: a **first-match visibility gate**
(allow/deny) is applied to each upstream skill, then a **layered skill upsert**
(rewrite-or-inject, keyed by `skill.id`) runs on the survivors. Reshaping is
per-caller-identity on the authenticated *extended* card and global on the
unauthenticated *public* card.

**Disclosure ≠ authorization.** Hiding a skill from the card does **not** prevent
its invocation if the id is guessed. Invocation-time skill authorization is a
separate, out-of-scope concern. This policy governs *discovery*, not *access*.

## 2. Position in the request lifecycle

Outbound / response-shaping. The request filter does minimal work (classify the
surface + variant, read caller identity on the extended surface) and threads a
`CardContext` forward; the response filter does the reshaping. The request is
**never blocked** — non-card requests continue with `None` and the response
passes through untouched.

The request→response handoff uses the `RequestData` channel (not a
stream-property store): `request_filter` returns
`Flow::Continue(Some(CardContext{surface, variant, identity}))`, and
`response_filter` receives it as `RequestData<Option<CardContext>>`.

```
                          ┌──────────────────────── REQUEST LEG ────────────────────────┐
  client ──▶ Flex ──▶ request_filter (lib.rs:request_filter)
                        headers_state: method, :path, A2A-Version (header|query)
                        POST+json?  → peek JSON-RPC `method` (into_body_state)
                        classify_surface (detect.rs) → Option<(Surface, Variant)>
                          None  → Flow::Continue(None)               (not a card fetch)
                          Some  → surface==Extended? read_identity(Authentication)
                                  Flow::Continue(Some(CardContext{surface,variant,identity}))
                          └───────────────────────────────────────────────────────────┘
                                        │  CardContext threaded via RequestData
                                        ▼
  upstream ──▶ Flex ──▶ response_filter (lib.rs:response_filter)   ┌──── RESPONSE LEG ────┐
                          RequestData::Continue(Some(ctx))? else return (pass through)
                          contains_body()? else return
                          --- HEADERS PHASE ---  remove `content-length`
                          into_body_state()
                          --- BODY PHASE ---     body = handler.body()
                            transform_card_body (lib.rs:transform_card_body)
                              → GovernorRules::govern (governor.rs)
                            Ok(None)      → leave body untouched
                            Ok(Some(b))   → set_body(b)         (governed card)
                            Err(_)        → set_body(fail_closed_body(variant))
                          └──────────────────────────────────────────────────────────────┘
  client ◀── governed / passed-through / fail-closed card
```

## 3. Crate layout (split-model)

Two roots: a **definition asset** (schema + Exchange metadata) and a **`-flex`
implementation** (Rust/wasm). Self-contained — **pdk-a2a Pattern B**: the A2A
protocol knowledge lives in-crate (`a2a.rs`, `detect.rs`); there is **no shared
A2A crate dependency**.

```
a2a-agent-card-skill-governor/
  a2a-agent-card-skill-governor-definition/   gcl.yaml, exchange.json, Makefile, README.md
  a2a-agent-card-skill-governor-flex/         Cargo.toml, Makefile, playground/, tests/
    src/
      lib.rs            entrypoint + request/response filters
      detect.rs         surface + variant classification
      governor.rs       rule-evaluation engine
      a2a.rs            AgentCard/Skill model + method/path consts
      generated/config.rs   deserialized config structs
```

| Module | Role | ~LOC |
|---|---|---|
| `src/lib.rs` | Entrypoint `configure`; `request_filter` (classify + identity), `response_filter` (split-flow reshape), pure `transform_card_body`, `fail_closed_body`, `read_identity`. All PDK I/O lives here. | 517 |
| `src/detect.rs` | Pure `classify_surface` → `Option<(Surface, Variant)>`; the `Variant` enum (Legacy / V1JsonRpc / V1HttpJson / PublicGet). No PDK deps. | 106 |
| `src/governor.rs` | `GovernorRules::compile` / `::govern`; `Surface`, `Identity`, `Audience`, `Target`; the `*`/`?` glob matcher. Pure, fully unit-testable. | 503 |
| `src/a2a.rs` | `Skill` model (typed governed fields + `#[serde(flatten)] extra` for round-trip), `is_agent_card`, method-name/path consts. Pure. | 64 |
| `src/generated/config.rs` | `Config`, `VisibilityRule`, `SkillRule`, `SkillPayload` — serde structs mirroring the schema (camelCase aliases). | 58 |
| `definition/gcl.yaml` | Config schema + metadata (`injectionPoint: outbound`, `assetTypes: a2a,a2av1`, `category: security`). No `format: dataweave`, no object-literal defaults. | 103 |

## 4. Card surfaces

All three surfaces carry the same `AgentCard`; only the card's location in the
response and identity availability differ. Card fetches are unary JSON — never
`text/event-stream`, so there is no SSE handling. gRPC card binding is out of
scope.

| Surface (`Variant`) | Request | Card location in response | Identity available? |
|---|---|---|---|
| Public well-known (`PublicGet`) | `GET /.well-known/agent-card.json` | body **is** the AgentCard | **No** — unauthenticated by design; identity rules no-op, only `any`-audience rules apply |
| Extended JSON-RPC (`Legacy` / `V1JsonRpc`) | `POST` with JSON-RPC `method` = `agent/getAuthenticatedExtendedCard` (Legacy) or `GetExtendedAgentCard` (V1, with `A2A-Version: 1.0`) | AgentCard is the JSON-RPC **`result`** | Yes (post-auth) |
| Extended HTTP+JSON (`V1HttpJson`) | `GET /extendedAgentCard` with `A2A-Version: 1.0` | body **is** the AgentCard (bare) | Yes (post-auth) |

V1-vs-Legacy is decided by the `A2A-Version: 1.0` signal (header or `?a2a-version=1.0`
query). The V1 JSON-RPC method name seen *without* the V1 signal degrades to
`Legacy` (`detect.rs:classify_surface`).

## 5. Rule engine (Model C)

`GovernorRules::compile` validates config once at load (WARN + drop on bad rules;
fail-closed at load only if the whole config fails to parse). `GovernorRules::govern`
runs per confirmed card, in two ordered stages.

**Audience matching** (`Audience::matches`): `any` ⇒ always true; on the **public
surface any identity-typed audience (`client`/`scope`/`tier`) is false** (no-op);
otherwise `client` matches `client_id` **or** `client_name`, `scope` matches
membership in `identity.scopes`, `tier` matches `identity.tier`. Absent `audienceType`
defaults to `any` (applies to everyone, including anonymous).

**Skill targeting** (`Target`): `skillId` ⇒ exact; else `skillIdPattern` ⇒ `*`/`?`
glob (in-crate matcher, no external crate; every string is a valid pattern —
matched verbatim, no compile step or malformed-pattern warning); else ⇒ matches
all skills (global rule).

### Stage 1 — Visibility gate (first-match, per skill)
For each upstream skill, the verdict starts at `defaultAllow` (default `true`).
Visibility rules are scanned in declaration order; the **first** rule whose
audience and target both match sets the verdict (`allow`/`deny`) and stops the
scan. The skill is kept iff the verdict is allow. Denied ids are recorded (to warn
on later deny-then-reinject). Setting `defaultAllow: false` turns the ruleset into
a strict allow-list.

### Stage 2 — Skill upsert (layered, on survivors)
For each upsert entry whose audience matches, keyed by `skill.id`:
- **Rewrite** — id found among survivors: each provided field overrides the
  existing one. **Array fields (`tags`/`examples`/`inputModes`/`outputModes`) are
  replaced wholesale, never merged.**
- **Inject** — id not among survivors: appended **only if both `name` and
  `description` are present** (else WARN + skip). If the id was denied earlier this
  response, a deny-then-reinject WARN is logged, but the inject still proceeds.

Upserts run on survivors only, so injected/governor-authored skills bypass the
visibility gate (trusted) and **injected/rewritten always win on id-collision**.

**Empty `visibility` + empty `skills` ⇒ full passthrough** — the card is untouched
and everyone sees all skills.

## 6. Identity model

Identity is built by `lib.rs:read_identity` **only** on the extended surface, and
**only** from the Anypoint `Authentication` injectable (no raw token / JWT
parsing). Fields:

| Field | Source |
|---|---|
| `client_id` | `AuthenticationData.client_id` |
| `client_name` | `AuthenticationData.client_name` |
| `scopes` | Custom property under the configurable `scopeClaimKey` (default `"scope"`); string is split on whitespace/commas, array-of-strings is flattened |
| `tier` | **Custom-property channel** — first non-empty of `sla-tier-name`, `sla-tier-id`, `tier`, `sla_tier`, `slaTier` |

**`tier` has no first-class `AuthenticationData` field.** The custom properties
`sla-tier-name` / `sla-tier-id` are propagated by the SLA-tier rate-limiting
policy (`rate_limit_sla`). Therefore `tier` audience rules **require an upstream
SLA-tier policy**; absent it, `tier` degrades to `None` and `tier` rules never
match. Missing fields degrade gracefully rather than failing.

## 7. Failure & security model

### Fail-closed is BODY-ONLY
`transform_card_body` returns `Ok(None)` for pass-through, `Ok(Some(bytes))` for a
governed body, and `Err(_)` **only after the body is confirmed to be a card** but
shaping fails (missing `skills[]`, unparseable skills, serialize error). On `Err`,
`response_filter` replaces the **card body** with a surface-appropriate error
envelope via `fail_closed_body` — **the HTTP `:status` is left as the upstream sent
it (normally 200)**. Non-card responses and JSON-RPC `error` envelopes pass through
untouched.

| Surface / variant | Fail-closed body | HTTP status |
|---|---|---|
| Extended JSON-RPC (`Legacy` / `V1JsonRpc`) | JSON-RPC envelope, `error.code = -32603` (INTERNAL) | left as upstream (200 by convention) |
| Extended HTTP+JSON (`V1HttpJson`) | `google.rpc.Status`-shaped body (`error.code: 500` as a **payload field**, ErrorInfo detail) | left as upstream |
| Public well-known (`PublicGet`) | plain `{"error": "..."}` JSON | left as upstream |

**Why body-only, not a status rewrite:** on the split flow (see §8) the `:status`
header is committed in the headers phase, *before* the body is read — and the
fail-closed decision can only be made after reading the body and finding a card
that failed to shape. A body-content-dependent status change is therefore
impossible on the flow the runtime supports; the one PDK state that would permit a
late status change is the combined state that hangs. **The security property holds
regardless:** the ungoverned card body is always overwritten with an error
envelope, so ungoverned skills never leak. Only status-code fidelity for the two
bare-HTTP surfaces is given up.

### Other postures
- **Non-JSON / no `skills[]` / JSON-RPC `error` result** — pass through untouched (debug log).
- **Config invalid at entrypoint** — fail-closed at load; the policy will not configure.
- **Control plane unreachable (local mode)** — must still serve; `unwrap_or` + WARN, no panic.
- **No-panic posture** — `fail_closed_body` falls back to a static byte string if the
  tiny fixed error object somehow fails to serialize; `set_body` failures are logged, not panicked.
- **Telemetry** — logger only: WARN on misconfig / id-collision / deny-then-reinject,
  debug on pass-through, ERROR on fail-closed. No `PolicyViolation` (this is
  disclosure shaping, not client blocking). Metrics counters deferred.

### disclosure ≠ authorization (restated)
Removing a skill from the card only removes it from **discovery**. A caller who
knows or guesses a skill id can still invoke it. This policy provides no
invocation-time enforcement.

## 8. Runtime constraints

- **Split headers→body response flow is load-bearing — do NOT reintroduce the
  combined state.** `response_filter` uses `into_headers_state()` →
  `into_body_state()`: `content-length` is removed in the headers phase, then the
  body is read and rewritten in the body phase (the gateway recomputes the length —
  the sanctioned pdk-request-headers-bodies pattern). The PDK **combined
  headers+body state (`into_headers_body_state`) hangs on the response leg on Flex
  1.12.1, returning an Envoy 504 on every response-transforming request.** This
  constraint is the reason fail-closed is body-only (§7). Any future edit that
  switches to the combined state to gain a late status rewrite will reintroduce the
  504.
- **PDK 1.9.0**, Rust compiled to `wasm32-wasip1`.
- **Build/test via `make`** (never raw `cargo`), per project workflow.

## 9. Out of scope

- **gRPC card binding** — protobuf reshaping too heavy; rare. JSON bindings only.
- **Invocation-time skill authorization** — separate follow-up idea; this policy
  shapes disclosure, not access.
- **Additive array merge** — array fields are replaced wholesale, never merged.
- **Direct JWT / token parsing** — identity comes from the `Authentication`
  injectable only.
- **Metrics counters** (cards governed / skills removed / rewritten / injected) —
  deferred to a later phase; current telemetry is logger-only.
