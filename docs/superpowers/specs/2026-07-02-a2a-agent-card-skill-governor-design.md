# A2A Agent Card Skill Governor — Design

- **Date:** 2026-07-02
- **Status:** Approved (architecture phase)
- **Idea:** [P4A idea 91fe698a](https://www.p4a.ai/dashboard/ideas/91fe698a-0b50-491b-a536-34dcaf73eebc)
- **Repo:** https://github.com/P4A-Policies-for-Agents/a2a-agent-card-skill-governor.git
- **PDK:** 1.9.0
- **Model:** split-model PDK custom policy (definition + `-flex` implementation)

## 1. Summary

A Flex/Omni Gateway **response-shaping** policy that governs the `skills[]` array
of an A2A **Agent Card** in-flight, before the card reaches the calling client.
It reshapes `skills[]` per caller identity (extended card) or globally (public
card) via operator-configured rules:

1. **Visibility** — allow/remove skills from discovery.
2. **Upsert** — rewrite an existing skill's fields, or inject a skill the upstream
   agent never declared. Keyed by `skill.id`: match ⇒ rewrite, no match ⇒ inject.

Out of scope (separate follow-up idea): invocation-time skill authorization.
Disclosure ≠ authorization — a hidden skill remains callable if its id is guessed.

## 2. Key design decisions

| # | Decision | Choice |
|---|---|---|
| Rule model | how rules combine | **Hybrid (C):** visibility resolved first-match (gate), then upsert layered on survivors |
| Interception | where the card is caught | **Auto-detect (C):** any JSON response whose body (or JSON-RPC `result`) is an AgentCard. JSON bindings only (no gRPC) for MVP |
| Identity | matcher source | **Anypoint `Authentication` injectable only.** No raw token parsing. Scope read from a configurable custom-property key |
| Public vs extended | identity availability | Public well-known card is **unauthenticated** — identity rules **no-op** there; only `any`-audience rules apply. Extended card runs post-auth — full per-identity governance |
| Array rewrite | merge vs replace | **Replace wholesale** — a provided array field overwrites entirely |
| Empty ruleset | posture | **Full passthrough** — card untouched, everyone sees all skills |
| Unmatched skill | visibility default | **Default-allow** (`defaultAllow: true`); strict allow-list via explicit trailing `deny` |
| Audience | absent audience | **Applies to everyone incl. anonymous** (implicit `any`) |
| Injection collision | injected id == survivor id | **Injected wins**, WARN logged |
| Failure mode | malformed / shaping failure | **Hybrid (C):** non-card passes through; confirmed card that fails shaping ⇒ **fail-closed** error |

## 3. Surfaces governed

All three A2A card surfaces carry the same `AgentCard`; the card lives at
different locations per binding:

| Surface | Request | Card location in response | Identity? |
|---|---|---|---|
| Public well-known | `GET /.well-known/agent-card.json` | body **is** the AgentCard | No (anonymous by design) |
| Extended (JSON-RPC) | `agent/getAuthenticatedExtendedCard` (Legacy) / `GetExtendedAgentCard` (V1) | JSON-RPC `result` **is** the AgentCard | Yes (post-auth) |
| Extended (HTTP+JSON) | `GET /extendedAgentCard` | body **is** the AgentCard | Yes (post-auth) |

gRPC card binding is out of scope for MVP (protobuf reshaping too heavy; rare).

Card fetches are unary JSON — never `text/event-stream`. No SSE handling needed.

## 4. Architecture & data flow

Response-shaping policy. The request filter does minimal work (classify + stash);
the response filter does the reshaping.

```
REQUEST filter:
  classify variant (Legacy | V1-JsonRpc | V1-HttpJson) + surface:
    - well-known GET path .................................. PUBLIC
    - getAuthenticatedExtendedCard / GetExtendedAgentCard
      / GET /extendedAgentCard ............................. EXTENDED
    - anything else ....................................... SKIP (not a card fetch)
  read Authentication -> {client_id, client_name, tier, scope[]}
  stash {variant, surface, identity} in stream properties
  Flow::Continue   (never blocks)

RESPONSE filter:
  if SKIP flag set -> pass through
  parse body:
    - JsonRpc variant: parse envelope; if `error` present -> pass through untouched;
      else take `result`
    - else: body is card directly
  if parsed JSON is not an object with a `skills` array -> pass through (not our target)
  --- GOVERN skills[] (see §5) ---
  re-serialize (re-wrap in JSON-RPC `result` if JsonRpc variant)
  on shaping failure of a confirmed card -> fail-closed error (see §6)
```

### Crate layout (split-model)

```
a2a-agent-card-skill-governor/
  a2a-agent-card-skill-governor-definition/
    gcl.yaml, exchange.json, Makefile, README.md
  a2a-agent-card-skill-governor-flex/
    src/
      lib.rs         # entrypoint, request+response filters
      detect.rs      # variant + surface classification
      a2a.rs         # AgentCard/Skill shape, method-name consts, inspectable subset
      governor.rs    # rule-evaluation engine
      generated/config.rs
    tests/
    playground/
    Cargo.toml, Makefile
```

Self-contained (pdk-a2a Pattern B) — no shared A2A crate dependency.

## 5. Configuration schema (gcl.yaml, typed model B)

No `format: dataweave` anywhere (matching is against `Authentication`, not payload
selectors) — avoids the array-item dataweave deploy-time crash. No object-literal
defaults — avoids the silent-passthrough trap.

```yaml
metadata:
  labels:
    title: A2A Agent Card Skill Governor
    description: Governs the skills[] of an A2A Agent Card in-flight per caller identity.
    category: security
    metadata/capabilities/injectionPoint: outbound
    metadata/capabilities/assetTypes: a2a,a2av1
spec:
  extends:
    - name: extension-definition
  properties:
    scopeClaimKey: { type: string, default: "scope" }   # Authentication custom-prop key for scope
    defaultAllow:  { type: boolean, default: true }      # unmatched-skill visibility posture

    # --- 1. VISIBILITY: first-match allow/deny (Model C gate) ---
    visibility:
      type: array
      items:
        type: object
        properties:
          effect:         { type: string, enum: [allow, deny] }
          audienceType:   { type: string, enum: [any, client, scope, tier] }
          audienceValue:  { type: string }
          skillId:        { type: string }
          skillIdPattern: { type: string }
        required: [effect]

    # --- 2. SKILLS UPSERT: rewrite existing OR inject new, keyed by skill.id ---
    skills:
      type: array
      items:
        type: object
        properties:
          audienceType:  { type: string, enum: [any, client, scope, tier] }
          audienceValue: { type: string }
          skill:
            type: object
            properties:
              id:          { type: string }   # key: match => rewrite, no-match => inject
              name:        { type: string }
              description: { type: string }
              tags:        { type: array, items: { type: string } }
              examples:    { type: array, items: { type: string } }
              inputModes:  { type: array, items: { type: string } }
              outputModes: { type: array, items: { type: string } }
            required: [id]
        required: [skill]
```

- Empty `visibility` + empty `skills` ⇒ full passthrough.
- `audienceType` defaults to `any` (everyone incl. anonymous). Identity types
  (`client`/`scope`/`tier`) no-op on the PUBLIC surface.
- Array fields are plain `Vec<String>`, replaced wholesale.

## 6. Rule evaluation engine

### Config-time (entrypoint, once) — validate & WARN

- `audienceType != any` but `audienceValue` empty ⇒ WARN, drop rule.
- Upsert entry with empty `skill.id` ⇒ WARN, drop.
- Malformed `skillIdPattern` glob ⇒ WARN, drop.
- Fail-closed at load if the whole config fails to parse (pdk-a2a contract).

### Per-response (confirmed AgentCard)

```
inputs: upstream_skills[], surface (PUBLIC|EXTENDED), identity {client_id, client_name, tier, scopes[]}

audience_matches(rule):
    any                        => true
    (PUBLIC surface, identity) => false        # no-op on public card
    client                     => identity.client_id == v || identity.client_name == v
    scope                      => v in identity.scopes
    tier                       => identity.tier == v

skill_target_matches(rule, skill):
    rule.skillId        => exact match
    else rule.skillIdPattern => glob match
    else                => matches all skills (global rule)

# 1. VISIBILITY (first-match per skill)
for skill in upstream_skills:
    verdict = defaultAllow ? ALLOW : DENY
    for rule in visibility (declaration order):
        if audience_matches(rule) && skill_target_matches(rule, skill):
            verdict = rule.effect; break         # first match wins
    keep skill iff verdict == ALLOW
survivors = kept skills

# 2. SKILLS UPSERT (layered, declaration order)
for entry in skills:
    if !audience_matches(entry): continue
    idx = survivors.find(id == entry.skill.id)
    if idx found:                                # REWRITE
        override each provided field (arrays wholesale)
    else:                                        # INJECT
        if entry.skill.name && entry.skill.description present:
            survivors.append(entry.skill)        # injected wins; append order
            (if id previously denied this response: WARN "deny-then-reinject")
        else:
            WARN "inject <id> missing name/description"; skip

card.skills = survivors
```

Notes:
- Upsert runs on **survivors only**. A deny removes upstream's version; a same-id
  upsert entry then *injects* a governor-authored replacement (intentional; WARN).
- Injected skills bypass the visibility gate (governor-authored ⇒ trusted).
- Injected/rewritten skills always win on id-collision.

## 7. Failure modes & telemetry

| Condition | Posture | Response |
|---|---|---|
| Non-JSON / no `skills[]` / JSON-RPC `error` result | pass through untouched | debug log |
| Confirmed card, shaping fails (serialize/internal error) | **fail-closed** | surface-aware error + ERROR log |
| Config invalid at entrypoint | fail-closed at load | policy will not configure |
| Control plane unreachable (local mode) | must still serve | no panic; `unwrap_or` + WARN |

Fail-closed error shape (surface-aware, per pdk-a2a error envelopes):
- EXTENDED JsonRpc (Legacy/V1) ⇒ JSON-RPC envelope, HTTP 200, `-32603 INTERNAL`.
- EXTENDED HTTP+JSON ⇒ `google.rpc.Status`, HTTP 500, reason `INTERNAL`.
- PUBLIC well-known ⇒ plain HTTP 500 (bare GET, not A2A-enveloped).

**Telemetry:** logger — WARN on misconfig / id-collision / deny-then-reinject;
debug on pass-through / non-card; ERROR on fail-closed. No PolicyViolation (this is
disclosure shaping, not client blocking). Metrics counters (cards governed, skills
removed / rewritten / injected) deferred to a later phase.

## 8. Testing strategy

- **Unit (pdk-unit):** rule engine — visibility first-match, default-allow/deny,
  audience matching per surface, wholesale array replace, upsert rewrite vs inject,
  id-collision, deny-then-reinject WARN, empty-ruleset passthrough. Local-mode
  no-panic path.
- **Integration (pdk-integration-tests):** all three surfaces end-to-end — public
  well-known GET, extended JSON-RPC (Legacy + V1), extended HTTP+JSON. Assert
  identity rules no-op on public. Non-card + JSON-RPC-error passthrough.
  Fail-closed on corrupted card.

## 9. Out of scope (MVP)

- gRPC card binding.
- Invocation-time skill authorization (separate idea).
- Additive array merge (replace-only).
- Direct JWT parsing (identity comes from `Authentication` only).
- Metrics counters (later phase).
