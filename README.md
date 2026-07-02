# A2A Agent Card Skill Governor

A MuleSoft Omni Gateway custom policy that governs the `skills[]` array of an [Agent-to-Agent (A2A)](https://google.github.io/A2A/) **Agent Card** in-flight, before the card reaches the calling client. It reshapes the advertised skill set per caller identity (on the authenticated extended card) or globally (on the public well-known card), driven entirely by operator configuration.

Built with the [Policy Development Kit (PDK)](https://docs.mulesoft.com/pdk/latest/policies-pdk-overview) as a standalone, split-model project. This is a **response-shaping, outbound-injection** policy.

## Focus

An A2A Agent Card is the agent's public résumé: it advertises the `skills[]` the agent can perform, and clients read it to decide what to invoke. The upstream agent authors one card, but not every caller should see the same skill set — an internal `admin.reindex` skill has no business appearing on the unauthenticated public card, a premium capability may belong only to a `gold`-tier client, and an operator may want to publish a governed description in place of the agent's own.

This policy is the **skill-disclosure control point at the A2A perimeter**. It intercepts the card on its way back to the caller and applies two mechanisms (the "hybrid" rule model):

1. **Visibility** — a first-match allow/deny gate. Each upstream skill is kept or removed from the card based on the first matching rule (or the `defaultAllow` posture when none match).
2. **Skills upsert** — rewrite an existing skill's fields, or inject a skill the upstream agent never declared, keyed by `skill.id` (match ⇒ rewrite, no match ⇒ inject).

Key properties:

- **Identity comes from the Anypoint `Authentication` injectable only.** The policy never parses a raw token. `client_id`, `client_name`, `tier`, and `scope` are read from the authentication data set by an upstream auth/contract policy. Scope is read from a configurable custom-property key (`scopeClaimKey`).
- **Three card surfaces, one behavior.** The same `AgentCard` shape is governed regardless of the wire binding it arrives on (see the surfaces table below).
- **Empty ruleset ⇒ full passthrough.** With no `visibility` and no `skills` rules configured, the card is returned byte-faithful — the policy is inert.
- **Fail-closed is body-only.** If a confirmed card cannot be shaped, the card body is replaced with a surface-appropriate error envelope so ungoverned skills never leak. The HTTP status is left as the upstream sent it (see [Failure behavior](#failure-behavior)).

> ## ⚠️ Disclosure is not authorization
>
> Removing a skill from the Agent Card hides it from **discovery** — it does not prevent **invocation**. A client that already knows (or guesses) a skill's `id` can still call it; this policy does not gate invocation-time requests. Treat card governance as attack-surface reduction and least-disclosure, **not** as an access-control boundary. Invocation-time skill authorization is a separate, out-of-scope concern.

## Project layout

```
a2a-agent-card-skill-governor/
├── a2a-agent-card-skill-governor-definition/   # GCL schema + Exchange metadata
│   ├── exchange.json
│   ├── gcl.yaml
│   └── Makefile
└── a2a-agent-card-skill-governor-flex/         # Rust implementation (compiles to wasm32-wasip1)
    ├── Cargo.toml
    ├── Makefile
    ├── src/
    │   ├── lib.rs              # entrypoint + request/response filter wiring
    │   ├── detect.rs           # card surface + A2A variant classification
    │   ├── a2a.rs              # AgentCard/Skill shape, method-name constants
    │   ├── governor.rs         # rule-evaluation engine (visibility + upsert)
    │   └── generated/          # config struct from gcl.yaml
    ├── playground/             # Docker-based local Omni Gateway for `make run`
    └── tests/                  # integration tests (pdk-test)
```

## Surfaces governed

All three A2A card surfaces carry the same `AgentCard`; only its location in the response differs. The policy detects the surface from the request and locates the card accordingly.

| Surface | Request | Card location in response | Identity? |
|---|---|---|---|
| Public well-known | `GET /.well-known/agent-card.json` | body **is** the AgentCard | No — unauthenticated by design |
| Extended (JSON-RPC) | `POST` `GetExtendedAgentCard` (V1, with `A2A-Version: 1.0`) or `agent/getAuthenticatedExtendedCard` (Legacy) | JSON-RPC `result` **is** the AgentCard | Yes — post-auth |
| Extended (HTTP+JSON) | `GET /extendedAgentCard` with `A2A-Version: 1.0` | body **is** the AgentCard | Yes — post-auth |

Notes:

- **V1 vs Legacy detection.** For the JSON-RPC extended card, the `A2A-Version: 1.0` signal (request header, or `?A2A-Version=1.0` query parameter) distinguishes the V1 binding (`GetExtendedAgentCard`) from Legacy (`agent/getAuthenticatedExtendedCard`). The V1 HTTP+JSON `GET /extendedAgentCard` binding requires the `A2A-Version: 1.0` signal.
- **Identity no-ops on the public card.** The well-known card is fetched anonymously, so identity-scoped rules (`client`/`scope`/`tier`) cannot bind there — only `any`-audience rules apply.
- **JSON responses only.** Card fetches are unary JSON; there is no `text/event-stream` / SSE handling, and the gRPC card binding is out of scope.
- Any response that is not JSON, does not carry a `skills[]` array, or is a JSON-RPC `error` envelope passes through untouched.

## Configuration reference

Configuration is set on the policy binding (`api.yaml` locally, or the API Manager policy config). Every property is optional; an empty configuration is full passthrough.

| Property | Type | Default | Description |
|---|---|---|---|
| `scopeClaimKey` | string | `"scope"` | The `Authentication` custom-property key that carries the caller's scope(s). Values may be a space/comma-separated string or an array of strings. |
| `defaultAllow` | boolean | `true` | Visibility posture for a skill that matches no `visibility` rule. `true` keeps it; `false` hides it (strict allow-list posture). |
| `visibility` | array | `[]` | Ordered allow/deny rules. **First match per skill wins.** See [visibility rule](#visibility-rule). |
| `skills` | array | `[]` | Skill upsert entries — rewrite an existing skill or inject a new one, keyed by `skill.id`. See [skills upsert entry](#skills-upsert-entry). |

### Visibility rule

| Field | Type | Description |
|---|---|---|
| `effect` | `allow` \| `deny` | **Required.** What to do with a skill this rule matches. |
| `audienceType` | `any` \| `client` \| `scope` \| `tier` | Who the rule applies to. Defaults to `any` (everyone, including anonymous). Identity types no-op on the public card. |
| `audienceValue` | string | The value to match for a non-`any` audience (a `client_id`/`client_name`, a scope, or a tier). An identity audience with an empty value is dropped with a WARN at load. |
| `skillId` | string | Match a single skill by exact `id`. |
| `skillIdPattern` | string | Match skills by glob (`*` / `?`) against `id`. Used only when `skillId` is absent. |

If neither `skillId` nor `skillIdPattern` is set, the rule matches **all** skills (a global gate). Audience matching:

- `any` → always matches.
- On the **public** surface, any identity-typed rule → never matches (no identity available).
- `client` → matches when `audienceValue` equals the caller's `client_id` **or** `client_name`.
- `scope` → matches when `audienceValue` is one of the caller's scopes.
- `tier` → matches when `audienceValue` equals the caller's SLA tier.

### Skills upsert entry

| Field | Type | Description |
|---|---|---|
| `audienceType` / `audienceValue` | (same as above) | Who this upsert applies to. Same matching rules. |
| `skill` | object | **Required.** The skill payload. `skill.id` is **required** and is the key. |

The `skill` payload carries `id` (required) plus any of `name`, `description`, `tags[]`, `examples[]`, `inputModes[]`, `outputModes[]`. On a **rewrite**, each provided field overwrites the upstream skill's field; array fields are **replaced wholesale**, not merged; fields you omit are left untouched. On an **inject**, the entry becomes a brand-new skill.

Upsert runs on the survivors of the visibility gate, in declaration order. An injected/rewritten skill always wins on `id` collision. Injecting requires both `name` and `description`; an inject entry missing either is skipped with a WARN.

### Worked examples

Each rule below is shown as it appears under the corresponding top-level key in the policy config.

**Allow** — strict allow-list: hide everything, then explicitly permit `search` (put the `allow` first so it wins the first-match race):

```yaml
defaultAllow: false          # hide any skill no rule matches
visibility:
  - effect: allow
    audienceType: any
    skillId: search
```

**Deny** — hide the `secret` skill from every caller on every surface:

```yaml
visibility:
  - effect: deny
    audienceType: any
    skillId: secret
  # glob variant: hide every admin.* skill
  - effect: deny
    audienceType: any
    skillIdPattern: "admin.*"
```

**Deny for one audience** — hide `billing.refund` from the `basic` tier only (gold/platinum still see it; no-op on the public card):

```yaml
visibility:
  - effect: deny
    audienceType: tier
    audienceValue: basic
    skillId: billing.refund
```

**Rewrite** — replace an existing skill's description with a governed one, leaving its `name` and `tags` intact:

```yaml
skills:
  - audienceType: any
    skill:
      id: search
      description: "Search the knowledge base (governed description)"
```

**Inject** — add a skill the upstream agent never declared, visible only to callers holding the `partner` scope:

```yaml
skills:
  - audienceType: scope
    audienceValue: partner
    skill:
      id: partner.export
      name: "Partner Export"
      description: "Bulk export available to partner integrations"
      tags: ["export", "partner"]
```

## Failure behavior

| Condition | Posture |
|---|---|
| Non-JSON body, no `skills[]` array, or a JSON-RPC `error` envelope | Pass through untouched (debug log). |
| Confirmed card that cannot be shaped (unparseable skills, serialize failure) | **Fail closed, body-only** — replace the card body with a surface-appropriate error envelope, leave `:status` as the upstream sent it. ERROR log. |
| Configuration invalid at load | Policy will not configure. |
| Misconfigured rule (empty audience value, empty `skill.id`, bad glob, inject missing name/description) | WARN at load / on match; the offending rule or inject is dropped. |

**Why fail-closed is body-only (not a status rewrite):** the response filter uses the split headers→body flow because the PDK combined headers+body state hangs on the response leg (Envoy 504) on the runtime version this policy targets. On the split flow the `:status` header is committed in the headers phase, *before* the body is read — but the fail-closed decision requires reading the body (a failure is only known after confirming a card and failing to shape it). A body-content-dependent status change is therefore impossible on the supported flow. The security property is preserved regardless: the ungoverned card body is always replaced with an error envelope, so skills never leak. Only status-code fidelity for the two bare-HTTP surfaces is given up.

The fail-closed error body per surface:

- **Extended JSON-RPC** (V1 / Legacy) → a JSON-RPC `-32603 INTERNAL` envelope (HTTP 200 was always the JSON-RPC in-band convention).
- **Extended HTTP+JSON** → a `google.rpc.Status`-shaped body where `error.code: 500` is a payload field, **not** the HTTP status.
- **Public well-known** → a plain JSON error body (`{"error": "..."}`).

## Build & test

Use the `make` targets in `a2a-agent-card-skill-governor-flex/`:

- `make build` — compile the WASM binary and package the policy.
- `make test-unit` — run unit tests only (no Docker required).
- `make test` — run unit + integration tests (requires Docker and a local registration; see the flex README).
- `make test-one TEST=<name>` — run a single test.
- `make run` — run the policy in a local Omni Gateway playground (see the local-testing skill under `a2a-agent-card-skill-governor-flex/.claude/test-a2a-agent-card-skill-governor-locally/`).

## License

Apache-2.0. See source headers.
