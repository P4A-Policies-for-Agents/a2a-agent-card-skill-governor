# A2A Agent Card Skill Governor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Flex/Omni Gateway split-model PDK policy that reshapes the `skills[]` array of an A2A Agent Card in-flight — per caller identity on the extended card, globally on the public card.

**Architecture:** Response-shaping policy. The request filter classifies the card surface/variant and reads Anypoint `Authentication`, threading a `CardContext` to the response filter via `RequestData`. The response filter detects an `AgentCard` in the body (unwrapping a JSON-RPC `result` when present), runs the rule engine (first-match visibility gate → layered skill upsert), re-serializes, and writes the body back. Non-cards pass through; a confirmed card that fails shaping fails closed.

**Tech Stack:** Rust + PDK 1.9.0 (`pdk`, `pdk-test`, `pdk-unit`), `serde` / `serde_json`, `anyhow`, `thiserror`. Self-contained (pdk-a2a Pattern B) — no shared A2A crate. Docker playground for local runs.

## Global Constraints

- **PDK version:** `pdk`, `pdk-test`, `pdk-unit` all `1.9.0`. `rust-version = "1.88.0"`, `edition = "2021"`, `resolver = "2"`.
- **Split-model layout:** `<root>-definition/` (gcl.yaml, exchange.json, Makefile) + `<root>-flex/` (Cargo.toml, Makefile, src/, tests/, playground/).
- **Build/test via `make`** — never invoke `cargo` directly.
- **Prefer PDK features** over std/third-party where a PDK equivalent exists.
- **`assetTypes: a2a,a2av1`** — exact spelling (`a2av1`, no underscore); `a2a_v1` fails Exchange publish.
- **`description` label ≤ 256 characters** (Exchange publish limit); long prose goes in README.
- **Terminology:** keep code/config/prose on "Flex" (toolchain not yet rebranded to Omni).
- **No `unwrap()`/`panic!`** on context-derived `Option`/`Result` — local mode must serve from t=0. Use `if let` / `match` / `unwrap_or` + logged warning.
- **License header** on every source file: `// Copyright 2026 Salesforce, Inc. All rights reserved.` / `// SPDX-License-Identifier: Apache-2.0`.
- **No `format: dataweave`** anywhere in gcl.yaml (matching is against `Authentication`, not payload). No object-literal defaults.
- **Spec:** `docs/superpowers/specs/2026-07-02-a2a-agent-card-skill-governor-design.md` — keep code and spec aligned; update docs in the same pass as code.

---

## File structure

```
a2a-agent-card-skill-governor/
  a2a-agent-card-skill-governor-definition/
    gcl.yaml            # policy schema (metadata + spec.properties)
    exchange.json       # Exchange asset descriptor
    Makefile            # publish targets
    README.md
  a2a-agent-card-skill-governor-flex/
    Cargo.toml
    Makefile
    src/
      lib.rs            # entrypoint, request+response filter wiring, body I/O
      detect.rs         # variant + surface classification, CardContext, Identity
      a2a.rs            # AgentCard / Skill types, method-name consts
      governor.rs       # rule engine: config model, visibility gate, skill upsert
      generated/
        mod.rs
        config.rs       # generated from gcl.yaml via `make build-asset-files`
    tests/
      requests.rs       # integration tests (Docker Flex, all surfaces)
    playground/         # Docker compose + api config for `make run`
```

Responsibility split:
- `a2a.rs` — pure data model (card + skill shapes, method-name constants). No PDK deps.
- `detect.rs` — surface/variant classification + `Identity` extraction glue types. Pure functions where possible.
- `governor.rs` — the rule engine. Pure, fully unit-testable, no PDK deps.
- `lib.rs` — the only file touching `pdk::hl` (filters, body I/O, entrypoint).

---

### Task 1: Scaffold split-model project (empty passthrough policy)

**Files:**
- Create: `a2a-agent-card-skill-governor-flex/Cargo.toml`
- Create: `a2a-agent-card-skill-governor-flex/Makefile`
- Create: `a2a-agent-card-skill-governor-flex/src/lib.rs`
- Create: `a2a-agent-card-skill-governor-flex/src/generated/mod.rs`
- Create: `a2a-agent-card-skill-governor-flex/src/generated/config.rs`
- Create: `a2a-agent-card-skill-governor-definition/gcl.yaml`
- Create: `a2a-agent-card-skill-governor-definition/exchange.json`
- Create: `a2a-agent-card-skill-governor-definition/Makefile`

**Interfaces:**
- Consumes: nothing.
- Produces: a buildable `cdylib` with a no-op `#[entrypoint] configure` that launches a passthrough `on_request().on_response()` filter. Later tasks add real logic inside these filters.

> **Preferred path:** scaffold with `anypoint-cli-v4 pdk policy-project create` (see the `pdk-create-policy` skill) to get canonical Makefiles/playground, then rename to the file paths above and overwrite `gcl.yaml`. The steps below give the exact file contents if generating by hand.

- [ ] **Step 1: Write `Cargo.toml`**

```toml
# Copyright 2026 Salesforce, Inc. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

[package]
name = "a_two_a_agent_card_skill_governor"
version = "0.1.0"
rust-version = "1.88.0"
edition = "2021"
resolver = "2"
description = "A2A Agent Card Skill Governor policy for MuleSoft Omni Gateway"
license = "Apache-2.0"

[lib]
crate-type = ["cdylib"]

[package.metadata.anypoint]
group_id = "00000000-0000-0000-0000-000000000000"
definition_asset_id = { name = "a-two-a-agent-card-skill-governor", version = "0.1.0" }
implementation_asset_id = "a-two-a-agent-card-skill-governor-flex"

[dependencies]
pdk = { version = "1.9.0", features = ["enable_stop_iteration"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = { version = "1.0", default-features = false, features = ["alloc", "raw_value"] }
anyhow = "1.0"
thiserror = "1.0"

[dev-dependencies]
pdk-test = { version = "1.9.0" }
pdk-unit = { version = "1.9.0" }
reqwest = { version = "0.11", features = ["json"] }

[profile.release]
lto = true
opt-level = 'z'
strip = "debuginfo"
```

- [ ] **Step 2: Write the passthrough `src/lib.rs`**

```rust
// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A2A Agent Card Skill Governor policy for MuleSoft Omni Gateway.
//!
//! Reshapes the `skills[]` array of an A2A Agent Card in-flight, per caller
//! identity (extended card) or globally (public card).

mod generated;

use anyhow::{anyhow, Result};
use pdk::hl::*;

use crate::generated::config::Config;

async fn request_filter(_request_state: RequestState) -> Flow<()> {
    Flow::Continue(())
}

async fn response_filter(_response_state: ResponseState, _request_data: RequestData<()>) {}

#[entrypoint]
async fn configure(launcher: Launcher, Configuration(bytes): Configuration) -> Result<()> {
    let _config: Config = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("Failed to parse configuration: {}", e))?;
    let filter = on_request(|rs| request_filter(rs)).on_response(|rs, rd| response_filter(rs, rd));
    launcher.launch(filter).await?;
    Ok(())
}
```

- [ ] **Step 3: Write `src/generated/mod.rs`**

```rust
// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
pub mod config;
```

- [ ] **Step 4: Write a minimal `src/generated/config.rs`** (regenerated in Task 2)

```rust
// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Config {}
```

- [ ] **Step 5: Write `a2a-agent-card-skill-governor-definition/gcl.yaml`** (minimal; full schema in Task 2)

```yaml
# Copyright 2026 Salesforce, Inc. All rights reserved.
# SPDX-License-Identifier: Apache-2.0
---
apiVersion: gateway.mulesoft.com/v1alpha1
kind: Extension
metadata:
  labels:
    title: A2A Agent Card Skill Governor
    description: Governs the skills[] of an A2A Agent Card in-flight per caller identity.
    category: security
    metadata/interfaceScope: api
    metadata/capabilities/injectionPoint: outbound
    metadata/capabilities/assetTypes: a2a,a2av1
spec:
  extends:
    - name: extension-definition
      namespace: default
  properties: {}
```

- [ ] **Step 6: Write `exchange.json` and both `Makefile`s** by copying the structure from the sibling `a2a-pii-guard-policy` definition/flex (adjust asset ids/names to `a-two-a-agent-card-skill-governor*`). Verify targets `build`, `build-asset-files`, `test`, `run` exist.

- [ ] **Step 7: Build**

Run: `cd a2a-agent-card-skill-governor-flex && make build`
Expected: compiles to `target/wasm32-wasip1/release/*.wasm`, no errors.

- [ ] **Step 8: Commit**

```bash
git add a2a-agent-card-skill-governor-flex a2a-agent-card-skill-governor-definition
git commit -m "feat: scaffold A2A Agent Card Skill Governor passthrough policy"
```

---

### Task 2: Config schema + generated config model

**Files:**
- Modify: `a2a-agent-card-skill-governor-definition/gcl.yaml` (full schema)
- Modify: `a2a-agent-card-skill-governor-flex/src/generated/config.rs` (via `make build-asset-files`)

**Interfaces:**
- Produces the generated `Config` struct consumed by Task 4/5/6:
  ```rust
  pub struct Config {
      pub scope_claim_key: Option<String>,   // default "scope"
      pub default_allow: Option<bool>,        // default true
      pub visibility: Option<Vec<VisibilityRule>>,
      pub skills: Option<Vec<SkillRule>>,
  }
  pub struct VisibilityRule {
      pub effect: String,                     // "allow" | "deny"
      pub audience_type: Option<String>,      // "any"|"client"|"scope"|"tier"
      pub audience_value: Option<String>,
      pub skill_id: Option<String>,
      pub skill_id_pattern: Option<String>,
  }
  pub struct SkillRule {
      pub audience_type: Option<String>,
      pub audience_value: Option<String>,
      pub skill: SkillPayload,
  }
  pub struct SkillPayload {
      pub id: String,
      pub name: Option<String>,
      pub description: Option<String>,
      pub tags: Option<Vec<String>>,
      pub examples: Option<Vec<String>>,
      pub input_modes: Option<Vec<String>>,
      pub output_modes: Option<Vec<String>>,
  }
  ```
  (Exact `#[serde(alias = ...)]` attributes are emitted by the generator; do not hand-edit.)

- [ ] **Step 1: Replace `spec.properties` in `gcl.yaml` with the full schema**

```yaml
  properties:
    scopeClaimKey:
      type: string
      title: Scope claim key
      description: Authentication custom-property key that carries the caller's scope(s).
      default: "scope"
    defaultAllow:
      type: boolean
      title: Default allow
      description: When no visibility rule matches a skill, allow it (true) or hide it (false).
      default: true
    visibility:
      type: array
      title: Visibility rules
      description: Ordered allow/deny rules. First match per skill wins.
      items:
        type: object
        properties:
          effect:         { type: string, title: Effect, enum: [allow, deny] }
          audienceType:   { type: string, title: Audience type, enum: [any, client, scope, tier] }
          audienceValue:  { type: string, title: Audience value }
          skillId:        { type: string, title: Skill id (exact) }
          skillIdPattern: { type: string, title: Skill id pattern (glob) }
        required: [effect]
    skills:
      type: array
      title: Skill upserts
      description: Rewrite an existing skill or inject a new one, keyed by skill.id.
      items:
        type: object
        properties:
          audienceType:  { type: string, title: Audience type, enum: [any, client, scope, tier] }
          audienceValue: { type: string, title: Audience value }
          skill:
            type: object
            title: Skill
            properties:
              id:          { type: string, title: Skill id }
              name:        { type: string, title: Name }
              description: { type: string, title: Description }
              tags:        { type: array, title: Tags, items: { type: string } }
              examples:    { type: array, title: Examples, items: { type: string } }
              inputModes:  { type: array, title: Input modes, items: { type: string } }
              outputModes: { type: array, title: Output modes, items: { type: string } }
            required: [id]
        required: [skill]
```

- [ ] **Step 2: Regenerate the config model**

Run: `cd a2a-agent-card-skill-governor-flex && make build-asset-files`
Expected: `src/generated/config.rs` now contains `Config`, `VisibilityRule`, `SkillRule`, `SkillPayload` structs matching the Interfaces block.

- [ ] **Step 3: Build to confirm the model compiles**

Run: `make build`
Expected: PASS. (The passthrough `configure` still ignores the config — wired up in Task 5.)

- [ ] **Step 4: Commit**

```bash
git add a2a-agent-card-skill-governor-definition/gcl.yaml a2a-agent-card-skill-governor-flex/src/generated/config.rs
git commit -m "feat: define full skill-governor config schema"
```

---

### Task 3: A2A card data model (`a2a.rs`)

**Files:**
- Create: `a2a-agent-card-skill-governor-flex/src/a2a.rs`
- Modify: `a2a-agent-card-skill-governor-flex/src/lib.rs` (add `mod a2a;`)

**Interfaces:**
- Produces (consumed by `governor.rs` and `lib.rs`):
  ```rust
  pub struct Skill {
      pub id: String,
      pub name: Option<String>,
      pub description: Option<String>,
      pub tags: Option<Vec<String>>,
      pub examples: Option<Vec<String>>,
      pub input_modes: Option<Vec<String>>,   // serde rename "inputModes"
      pub output_modes: Option<Vec<String>>,  // serde rename "outputModes"
      #[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>,
  }
  // method-name constants for extended-card detection
  pub const EXT_CARD_LEGACY: &str = "agent/getAuthenticatedExtendedCard";
  pub const EXT_CARD_V1: &str = "GetExtendedAgentCard";
  pub const WELL_KNOWN_PATH: &str = "/.well-known/agent-card.json";
  pub const EXT_CARD_HTTPJSON_PATH: &str = "/extendedAgentCard";
  // detect whether an arbitrary JSON value is an AgentCard
  pub fn is_agent_card(v: &serde_json::Value) -> bool;
  ```
  `Skill` uses `#[serde(flatten)] extra` so unknown card/skill fields round-trip untouched; only the governed fields are typed. `skip_serializing_if = "Option::is_none"` on every optional so omitted fields don't reappear as `null`.

- [ ] **Step 1: Write the failing test** (append to `a2a.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_agent_card_by_skills_array() {
        let card = json!({ "name": "Agent", "skills": [] });
        assert!(is_agent_card(&card));
        let not_card = json!({ "jsonrpc": "2.0", "result": 1 });
        assert!(!is_agent_card(&not_card));
        let skills_not_array = json!({ "skills": "nope" });
        assert!(!is_agent_card(&skills_not_array));
    }

    #[test]
    fn skill_roundtrips_unknown_fields() {
        let raw = json!({ "id": "s1", "name": "X", "securityRequirements": ["oauth"] });
        let s: Skill = serde_json::from_value(raw.clone()).unwrap();
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["securityRequirements"], json!(["oauth"]));
        assert!(back.get("description").is_none()); // omitted, not null
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd a2a-agent-card-skill-governor-flex && make test` (or `cargo test --lib a2a` if the Makefile forwards args)
Expected: FAIL — `Skill` / `is_agent_card` not defined.

- [ ] **Step 3: Write `a2a.rs`**

```rust
// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A2A Agent Card data model and card-detection helpers. Pure — no PDK deps.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const EXT_CARD_LEGACY: &str = "agent/getAuthenticatedExtendedCard";
pub const EXT_CARD_V1: &str = "GetExtendedAgentCard";
pub const WELL_KNOWN_PATH: &str = "/.well-known/agent-card.json";
pub const EXT_CARD_HTTPJSON_PATH: &str = "/extendedAgentCard";

/// One A2A Agent Card skill. Governed fields are typed; every other field
/// round-trips through `extra` so the rest of the card is preserved verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    #[serde(rename = "inputModes", skip_serializing_if = "Option::is_none")]
    pub input_modes: Option<Vec<String>>,
    #[serde(rename = "outputModes", skip_serializing_if = "Option::is_none")]
    pub output_modes: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// True when `v` looks like an AgentCard: a JSON object whose `skills` field
/// is an array.
pub fn is_agent_card(v: &Value) -> bool {
    v.get("skills").map(|s| s.is_array()).unwrap_or(false)
}
```

- [ ] **Step 4: Add `mod a2a;` to `lib.rs`** (below `mod generated;`)

- [ ] **Step 5: Run tests to verify they pass**

Run: `make test`
Expected: PASS (both `a2a` tests).

- [ ] **Step 6: Commit**

```bash
git add a2a-agent-card-skill-governor-flex/src/a2a.rs a2a-agent-card-skill-governor-flex/src/lib.rs
git commit -m "feat: add A2A card/skill data model and card detection"
```

---

### Task 4: Rule engine (`governor.rs`)

The heart of the policy. Pure functions, fully unit-testable, no PDK deps.

**Files:**
- Create: `a2a-agent-card-skill-governor-flex/src/governor.rs`
- Modify: `a2a-agent-card-skill-governor-flex/src/lib.rs` (add `mod governor;`)

**Interfaces:**
- Consumes: `crate::a2a::Skill`; the generated config types from Task 2 (`VisibilityRule`, `SkillRule`, `SkillPayload`).
- Produces (consumed by `lib.rs` in Task 6):
  ```rust
  pub enum Surface { Public, Extended }
  #[derive(Default)]
  pub struct Identity {
      pub client_id: Option<String>,
      pub client_name: Option<String>,
      pub tier: Option<String>,
      pub scopes: Vec<String>,
  }
  pub struct GovernorRules { /* validated, compiled from Config */ }
  impl GovernorRules {
      // Validates config, logs WARN on misconfig, drops bad rules. Called once at configure time.
      pub fn compile(config: &crate::generated::config::Config, warn: &mut dyn FnMut(String)) -> Self;
      // Reshapes skills for one card. `warn` collects runtime WARNs (collision, deny-reinject).
      pub fn govern(&self, skills: Vec<Skill>, surface: Surface, id: &Identity, warn: &mut dyn FnMut(String)) -> Vec<Skill>;
  }
  ```
  `compile` takes a `warn` sink so tests assert on warnings and `lib.rs` routes them to `pdk::logger`. Glob matching for `skill_id_pattern`: implement a tiny `*`/`?` matcher inline (no new dependency) — YAGNI on a glob crate.

- [ ] **Step 1: Write failing tests** (append to `governor.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::Skill;
    use crate::generated::config::*;
    use serde_json::Map;

    fn skill(id: &str) -> Skill {
        Skill { id: id.into(), name: Some(id.into()), description: None, tags: None,
                examples: None, input_modes: None, output_modes: None, extra: Map::new() }
    }
    fn cfg(vis: Vec<VisibilityRule>, sk: Vec<SkillRule>, default_allow: bool) -> Config {
        Config { scope_claim_key: None, default_allow: Some(default_allow),
                 visibility: Some(vis), skills: Some(sk) }
    }
    fn vis(effect: &str, at: &str, av: Option<&str>, sid: Option<&str>) -> VisibilityRule {
        VisibilityRule { effect: effect.into(), audience_type: Some(at.into()),
            audience_value: av.map(Into::into), skill_id: sid.map(Into::into), skill_id_pattern: None }
    }
    fn nowarn() -> impl FnMut(String) { |_| {} }
    fn anon() -> Identity { Identity::default() }

    #[test]
    fn empty_ruleset_is_full_passthrough() {
        let c = Config { scope_claim_key: None, default_allow: Some(true),
                         visibility: Some(vec![]), skills: Some(vec![]) };
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("a"), skill("b")], Surface::Public, &anon(), &mut w);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn deny_removes_skill_first_match_wins() {
        let c = cfg(vec![vis("deny", "any", None, Some("a")), vis("allow", "any", None, Some("a"))],
                    vec![], true);
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("a"), skill("b")], Surface::Extended, &anon(), &mut w);
        assert_eq!(out.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), vec!["b"]);
    }

    #[test]
    fn default_deny_hides_unmatched() {
        let c = cfg(vec![vis("allow", "any", None, Some("a"))], vec![], false);
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("a"), skill("b")], Surface::Extended, &anon(), &mut w);
        assert_eq!(out.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), vec!["a"]);
    }

    #[test]
    fn identity_rule_noops_on_public_surface() {
        // deny client=acme on a: must NOT apply on the public (unauthenticated) card
        let c = cfg(vec![vis("deny", "client", Some("acme"), Some("a"))], vec![], true);
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("a")], Surface::Public, &anon(), &mut w);
        assert_eq!(out.len(), 1); // survived — identity rule ignored on public
    }

    #[test]
    fn scope_rule_matches_on_extended() {
        let c = cfg(vec![vis("deny", "scope", Some("admin"), Some("a"))], vec![], true);
        let id = Identity { scopes: vec!["admin".into()], ..Default::default() };
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("a")], Surface::Extended, &id, &mut w);
        assert!(out.is_empty());
    }

    #[test]
    fn upsert_rewrites_existing_wholesale_arrays() {
        let sr = SkillRule { audience_type: Some("any".into()), audience_value: None,
            skill: SkillPayload { id: "a".into(), name: Some("New".into()), description: None,
                tags: Some(vec!["x".into()]), examples: None, input_modes: None, output_modes: None } };
        let c = cfg(vec![], vec![sr], true);
        let mut base = skill("a");
        base.tags = Some(vec!["old1".into(), "old2".into()]);
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![base], Surface::Extended, &anon(), &mut w);
        assert_eq!(out[0].name.as_deref(), Some("New"));
        assert_eq!(out[0].tags, Some(vec!["x".into()])); // replaced wholesale, not merged
    }

    #[test]
    fn upsert_injects_new_skill_appended() {
        let sr = SkillRule { audience_type: Some("any".into()), audience_value: None,
            skill: SkillPayload { id: "new".into(), name: Some("N".into()),
                description: Some("D".into()), tags: None, examples: None,
                input_modes: None, output_modes: None } };
        let c = cfg(vec![], vec![sr], true);
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("a")], Surface::Extended, &anon(), &mut w);
        assert_eq!(out.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), vec!["a", "new"]);
    }

    #[test]
    fn inject_missing_name_or_description_warns_and_skips() {
        let sr = SkillRule { audience_type: Some("any".into()), audience_value: None,
            skill: SkillPayload { id: "bad".into(), name: None, description: None, tags: None,
                examples: None, input_modes: None, output_modes: None } };
        let c = cfg(vec![], vec![sr], true);
        let mut warnings = Vec::new();
        let g = GovernorRules::compile(&c, &mut |m| warnings.push(m));
        let out = g.govern(vec![skill("a")], Surface::Extended, &anon(), &mut |m| warnings.push(m));
        assert_eq!(out.len(), 1); // "bad" not injected
        assert!(warnings.iter().any(|m| m.contains("bad") && m.contains("name")));
    }

    #[test]
    fn glob_pattern_denies_matching_skills() {
        let mut r = vis("deny", "any", None, None);
        r.skill_id_pattern = Some("admin.*".into());
        let c = cfg(vec![r], vec![], true);
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("admin.reset"), skill("user.read")], Surface::Extended, &anon(), &mut w);
        assert_eq!(out.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), vec!["user.read"]);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `make test`
Expected: FAIL — `GovernorRules` / `Surface` / `Identity` not defined.

- [ ] **Step 3: Implement `governor.rs`**

```rust
// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Rule-evaluation engine. Pure — no PDK deps, fully unit-testable.
//! Model C: first-match visibility gate, then layered skill upsert.

use crate::a2a::Skill;
use crate::generated::config::{Config, SkillRule, VisibilityRule};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Surface { Public, Extended }

#[derive(Default, Clone)]
pub struct Identity {
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    pub tier: Option<String>,
    pub scopes: Vec<String>,
}

enum Audience { Any, Client(String), Scope(String), Tier(String) }

enum Target { All, Exact(String), Glob(String) }

struct CompiledVis { effect_allow: bool, audience: Audience, target: Target }
struct CompiledUpsert { audience: Audience, skill: crate::generated::config::SkillPayload }

pub struct GovernorRules {
    default_allow: bool,
    visibility: Vec<CompiledVis>,
    upserts: Vec<CompiledUpsert>,
}

fn compile_audience(at: Option<&str>, av: Option<&str>, warn: &mut dyn FnMut(String)) -> Option<Audience> {
    match at.unwrap_or("any") {
        "any" => Some(Audience::Any),
        kind @ ("client" | "scope" | "tier") => match av {
            Some(v) if !v.is_empty() => Some(match kind {
                "client" => Audience::Client(v.into()),
                "scope" => Audience::Scope(v.into()),
                _ => Audience::Tier(v.into()),
            }),
            _ => { warn(format!("rule with audienceType '{kind}' missing audienceValue — dropped")); None }
        },
        other => { warn(format!("unknown audienceType '{other}' — dropped")); None }
    }
}

fn compile_target(id: Option<&str>, pat: Option<&str>) -> Target {
    match (id, pat) {
        (Some(i), _) if !i.is_empty() => Target::Exact(i.into()),
        (_, Some(p)) if !p.is_empty() => Target::Glob(p.into()),
        _ => Target::All,
    }
}

/// Minimal `*` / `?` glob matcher (no external crate). `*` = any run, `?` = one char.
fn glob_match(pat: &str, s: &str) -> bool {
    fn rec(p: &[u8], s: &[u8]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(b'*') => rec(&p[1..], s) || (!s.is_empty() && rec(p, &s[1..])),
            Some(b'?') => !s.is_empty() && rec(&p[1..], &s[1..]),
            Some(&c) => !s.is_empty() && s[0] == c && rec(&p[1..], &s[1..]),
        }
    }
    rec(pat.as_bytes(), s.as_bytes())
}

impl Audience {
    fn matches(&self, surface: Surface, id: &Identity) -> bool {
        match self {
            Audience::Any => true,
            // identity rules never bind on the unauthenticated public card
            _ if surface == Surface::Public => false,
            Audience::Client(v) => id.client_id.as_deref() == Some(v) || id.client_name.as_deref() == Some(v),
            Audience::Scope(v) => id.scopes.iter().any(|s| s == v),
            Audience::Tier(v) => id.tier.as_deref() == Some(v),
        }
    }
}

impl Target {
    fn matches(&self, skill_id: &str) -> bool {
        match self {
            Target::All => true,
            Target::Exact(i) => i == skill_id,
            Target::Glob(p) => glob_match(p, skill_id),
        }
    }
}

impl GovernorRules {
    pub fn compile(config: &Config, warn: &mut dyn FnMut(String)) -> Self {
        let default_allow = config.default_allow.unwrap_or(true);
        let mut visibility = Vec::new();
        for r in config.visibility.iter().flatten() {
            let VisibilityRule { effect, audience_type, audience_value, skill_id, skill_id_pattern } = r;
            let effect_allow = match effect.as_str() {
                "allow" => true, "deny" => false,
                other => { warn(format!("unknown visibility effect '{other}' — dropped")); continue }
            };
            let Some(audience) = compile_audience(audience_type.as_deref(), audience_value.as_deref(), warn) else { continue };
            visibility.push(CompiledVis { effect_allow, audience,
                target: compile_target(skill_id.as_deref(), skill_id_pattern.as_deref()) });
        }
        let mut upserts = Vec::new();
        for r in config.skills.iter().flatten() {
            let SkillRule { audience_type, audience_value, skill } = r;
            if skill.id.is_empty() { warn("skill upsert with empty id — dropped".into()); continue }
            let Some(audience) = compile_audience(audience_type.as_deref(), audience_value.as_deref(), warn) else { continue };
            upserts.push(CompiledUpsert { audience, skill: skill.clone() });
        }
        GovernorRules { default_allow, visibility, upserts }
    }

    pub fn govern(&self, skills: Vec<Skill>, surface: Surface, id: &Identity, warn: &mut dyn FnMut(String)) -> Vec<Skill> {
        // 1. Visibility gate — first-match per skill.
        let mut denied_ids: Vec<String> = Vec::new();
        let mut survivors: Vec<Skill> = skills.into_iter().filter(|s| {
            let mut verdict = self.default_allow;
            for r in &self.visibility {
                if r.audience.matches(surface, id) && r.target.matches(&s.id) {
                    verdict = r.effect_allow; break;
                }
            }
            if !verdict { denied_ids.push(s.id.clone()); }
            verdict
        }).collect();

        // 2. Skill upsert — layered, declaration order.
        for u in &self.upserts {
            if !u.audience.matches(surface, id) { continue }
            match survivors.iter_mut().find(|s| s.id == u.skill.id) {
                Some(existing) => apply_rewrite(existing, &u.skill),
                None => {
                    if u.skill.name.is_some() && u.skill.description.is_some() {
                        if denied_ids.iter().any(|d| d == &u.skill.id) {
                            warn(format!("skill '{}' was denied then re-injected", u.skill.id));
                        }
                        survivors.push(payload_to_skill(&u.skill));
                    } else {
                        warn(format!("inject '{}' missing name/description — skipped", u.skill.id));
                    }
                }
            }
        }
        survivors
    }
}

fn apply_rewrite(dst: &mut Skill, src: &crate::generated::config::SkillPayload) {
    if let Some(v) = &src.name { dst.name = Some(v.clone()); }
    if let Some(v) = &src.description { dst.description = Some(v.clone()); }
    if let Some(v) = &src.tags { dst.tags = Some(v.clone()); }
    if let Some(v) = &src.examples { dst.examples = Some(v.clone()); }
    if let Some(v) = &src.input_modes { dst.input_modes = Some(v.clone()); }
    if let Some(v) = &src.output_modes { dst.output_modes = Some(v.clone()); }
}

fn payload_to_skill(p: &crate::generated::config::SkillPayload) -> Skill {
    Skill {
        id: p.id.clone(), name: p.name.clone(), description: p.description.clone(),
        tags: p.tags.clone(), examples: p.examples.clone(),
        input_modes: p.input_modes.clone(), output_modes: p.output_modes.clone(),
        extra: serde_json::Map::new(),
    }
}
```

> If the generated `SkillPayload`/`SkillRule` field names differ from Task 2's Interfaces block (e.g. the generator emits `pii_type`-style aliases), adjust the field accesses here to match `src/generated/config.rs` exactly — the generated file is authoritative.

- [ ] **Step 4: Add `mod governor;` to `lib.rs`**

- [ ] **Step 5: Run tests to verify they pass**

Run: `make test`
Expected: PASS — all 9 governor tests.

- [ ] **Step 6: Commit**

```bash
git add a2a-agent-card-skill-governor-flex/src/governor.rs a2a-agent-card-skill-governor-flex/src/lib.rs
git commit -m "feat: implement skill-governor rule engine"
```

---

### Task 5: Surface/variant detection + request filter (`detect.rs` + wiring)

**Files:**
- Create: `a2a-agent-card-skill-governor-flex/src/detect.rs`
- Modify: `a2a-agent-card-skill-governor-flex/src/lib.rs` (request filter, `RequestData` threading, `mod detect;`)

**Interfaces:**
- Consumes: `crate::governor::{Surface, Identity}`, `crate::a2a` consts.
- Produces:
  ```rust
  #[derive(Clone)]
  pub enum Variant { Legacy, V1JsonRpc, V1HttpJson, PublicGet }
  #[derive(Clone)]
  pub struct CardContext { pub surface: Surface, pub variant: Variant, pub identity: Identity }
  // classify from request signals; None => not a card fetch (skip)
  pub fn classify(method: &str, path: &str, is_v1: bool, jsonrpc_method: Option<&str>) -> Option<CardContext_without_identity>;
  ```
  `CardContext` is the value threaded through `Flow::Continue(CardContext)` so the response filter (Task 6) receives it as `RequestData<CardContext>`. Because `Surface` needs no PDK types, `CardContext` derives `Clone`.

> **Note on `Surface`/`Identity` needing `Clone`/`PartialEq`:** ensure `governor.rs` derives are `#[derive(Clone, Copy, PartialEq, Eq)]` on `Surface` and `#[derive(Default, Clone)]` on `Identity` (already specified in Task 4).

- [ ] **Step 1: Write failing tests** (append to `detect.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_get_is_public() {
        let c = classify_surface("GET", "/.well-known/agent-card.json", false, None).unwrap();
        assert!(matches!(c, (Surface::Public, Variant::PublicGet)));
    }
    #[test]
    fn legacy_rpc_ext_card_is_extended() {
        let c = classify_surface("POST", "/", false, Some("agent/getAuthenticatedExtendedCard")).unwrap();
        assert!(matches!(c, (Surface::Extended, Variant::Legacy)));
    }
    #[test]
    fn v1_rpc_ext_card_is_extended() {
        let c = classify_surface("POST", "/", true, Some("GetExtendedAgentCard")).unwrap();
        assert!(matches!(c, (Surface::Extended, Variant::V1JsonRpc)));
    }
    #[test]
    fn v1_httpjson_ext_card_is_extended() {
        let c = classify_surface("GET", "/extendedAgentCard", true, None).unwrap();
        assert!(matches!(c, (Surface::Extended, Variant::V1HttpJson)));
    }
    #[test]
    fn unrelated_request_is_skip() {
        assert!(classify_surface("POST", "/", false, Some("message/send")).is_none());
        assert!(classify_surface("GET", "/health", false, None).is_none());
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `make test` → FAIL (`classify_surface` undefined).

- [ ] **Step 3: Implement `detect.rs`**

```rust
// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Card-surface / A2A-variant classification. Pure — no PDK deps.

use crate::a2a::{EXT_CARD_HTTPJSON_PATH, EXT_CARD_LEGACY, EXT_CARD_V1, WELL_KNOWN_PATH};
use crate::governor::Surface;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Variant { Legacy, V1JsonRpc, V1HttpJson, PublicGet }

fn path_no_query(path: &str) -> &str { path.split('?').next().unwrap_or(path) }

/// Returns (surface, variant) for a card-fetch request, or None if the
/// request is not an Agent Card fetch (response filter then passes through).
pub fn classify_surface(
    method: &str, path: &str, is_v1: bool, jsonrpc_method: Option<&str>,
) -> Option<(Surface, Variant)> {
    let p = path_no_query(path);
    // Public well-known card — unauthenticated GET.
    if method == "GET" && p.ends_with(WELL_KNOWN_PATH) {
        return Some((Surface::Public, Variant::PublicGet));
    }
    // Extended card via HTTP+JSON binding — GET /extendedAgentCard.
    if method == "GET" && is_v1 && p.ends_with(EXT_CARD_HTTPJSON_PATH) {
        return Some((Surface::Extended, Variant::V1HttpJson));
    }
    // Extended card via JSON-RPC — method string names it.
    match jsonrpc_method {
        Some(EXT_CARD_LEGACY) => Some((Surface::Extended, Variant::Legacy)),
        Some(EXT_CARD_V1) => Some((Surface::Extended, if is_v1 { Variant::V1JsonRpc } else { Variant::Legacy })),
        _ => None,
    }
}
```

- [ ] **Step 4: Wire the request filter in `lib.rs`**

Add `mod detect;`. Replace the passthrough `request_filter`/`response_filter` signatures so the request filter threads a `CardContext` (or `()` when skipping). Read `is_v1` from the `A2A-Version` header/query (pdk-a2a §detection), the JSON-RPC method from the POST body when content-type is JSON, and identity from the `Authentication` injectable.

```rust
mod detect;

use crate::detect::{classify_surface, Variant};
use crate::governor::{GovernorRules, Identity, Surface};

#[derive(Clone)]
pub struct CardContext { pub surface: Surface, pub variant: Variant, pub identity: Identity }

async fn request_filter(
    request_state: RequestState,
    rules: &GovernorRules,
    scope_claim_key: &str,
    auth: &Authentication,          // injected; see pdk-authentication
) -> Flow<Option<CardContext>> {
    let headers = request_state.into_headers_state().await;
    let h = headers.handler();
    let method = headers.method().as_str().to_string();
    let path = h.header(":path").unwrap_or_default();
    let is_v1 = a2a_version_is_v1(&h, &path);

    // Peek the JSON-RPC method only for POST + JSON bodies.
    let jsonrpc_method = if method == "POST"
        && h.header("content-type").map(|c| c.contains("json")).unwrap_or(false)
    {
        let body = headers.into_body_state().await;
        read_jsonrpc_method(body.as_bytes().as_slice())
    } else { None };

    let Some((surface, variant)) = classify_surface(&method, &path, is_v1, jsonrpc_method.as_deref())
    else { return Flow::Continue(None) };  // not a card fetch → response filter skips

    let identity = if surface == Surface::Extended {
        read_identity(auth, scope_claim_key)  // no-op values on public
    } else { Identity::default() };

    let _ = rules; // rules are applied on the response side
    Flow::Continue(Some(CardContext { surface, variant, identity }))
}
```

Provide the helpers (`a2a_version_is_v1`, `read_jsonrpc_method`, `read_identity`). `read_jsonrpc_method` parses just the `"method"` field with a borrowed `serde_json` struct (see the sibling `a2a-pii-guard` `JsonRpcRequest`). `read_identity` pulls `client_id`/`client_name`/tier and the scope custom-property from the `Authentication` injectable per the `pdk-authentication` skill; split the scope string on whitespace/comma into `scopes`.

> Consult skills before writing the PDK-touching helpers: **pdk-authentication** (reading principal/client_id/custom props), **pdk-a2a** (`A2A-Version` detection, borrowed JSON-RPC parse), **pdk-request-headers-bodies** (`:path` pseudo-header, body state), **pdk-metadata** (SLA tier if tier is sourced from `ApiMetadata` rather than a claim).

- [ ] **Step 5: Update the entrypoint** to `compile` rules once and inject `Authentication`:

```rust
#[entrypoint]
async fn configure(
    launcher: Launcher,
    Configuration(bytes): Configuration,
    auth: Authentication,
) -> Result<()> {
    let config: Config = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("Failed to parse configuration: {}", e))?;
    let scope_claim_key = config.scope_claim_key.clone().unwrap_or_else(|| "scope".into());
    let rules = GovernorRules::compile(&config, &mut |m| pdk::logger::warn!("[skill-governor] {}", m));

    let filter = on_request(|rs| request_filter(rs, &rules, &scope_claim_key, &auth))
        .on_response(|rs, rd| response_filter(rs, rd, &rules));  // response_filter body lands in Task 6
    launcher.launch(filter).await?;
    Ok(())
}
```

- [ ] **Step 6: Run tests** (`make test`) — detect unit tests PASS. `make build` compiles.

- [ ] **Step 7: Commit**

```bash
git add a2a-agent-card-skill-governor-flex/src/detect.rs a2a-agent-card-skill-governor-flex/src/lib.rs
git commit -m "feat: classify card surface/variant and thread request context"
```

---

### Task 6: Response filter — detect card, govern, write body, fail-closed

**Files:**
- Modify: `a2a-agent-card-skill-governor-flex/src/lib.rs` (response filter body)

**Interfaces:**
- Consumes: `RequestData<Option<CardContext>>`, `GovernorRules`, `crate::a2a::{Skill, is_agent_card}`.
- Produces: the shipped response body. No new public types.

Behavior (spec §4, §6, §7):
1. If `RequestData` is not `Continue(Some(ctx))` → return (pass through). This covers non-card requests and the request filter's `None`.
2. Read the response body. Parse JSON. For JSON-RPC variants (`Legacy`/`V1JsonRpc`): if the envelope has `error`, pass through untouched; else operate on `result`. For `PublicGet`/`V1HttpJson`: the body is the card directly.
3. If the target JSON is not an AgentCard (`is_agent_card` false) → pass through untouched.
4. Deserialize `skills` into `Vec<Skill>`, run `rules.govern(...)`, splice the governed array back into the card JSON (preserve all other card fields), re-wrap in the JSON-RPC `result` if applicable, serialize, and set the response body.
5. **Fail-closed** on any step that fails *after* the body is confirmed to be a card (serialize error, body-write error): replace the response with a surface-appropriate error (§7) and ERROR-log.

- [ ] **Step 1: Write failing tests** (append a `#[cfg(test)] mod resp_tests` in `lib.rs` for the pure body-transform helper)

Extract the pure transform into a testable free function so it needs no PDK harness:

```rust
// pure: takes body bytes + variant + governed decision, returns new body bytes or Err
#[cfg(test)]
mod resp_tests {
    use super::*;
    use crate::detect::Variant;
    use crate::governor::{GovernorRules, Identity, Surface};
    use crate::generated::config::*;

    fn deny_all_rules() -> GovernorRules {
        let c = Config { scope_claim_key: None, default_allow: Some(false),
            visibility: Some(vec![]), skills: Some(vec![]) };
        GovernorRules::compile(&c, &mut |_| {})
    }

    #[test]
    fn public_card_body_skills_removed() {
        let body = br#"{"name":"A","skills":[{"id":"a","name":"A"}]}"#;
        let out = transform_card_body(body, &Variant::PublicGet, &deny_all_rules(),
                                      Surface::Public, &Identity::default(), &mut |_| {}).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["skills"].as_array().unwrap().len(), 0);
        assert_eq!(v["name"], "A"); // untouched
    }

    #[test]
    fn jsonrpc_error_passes_through() {
        let body = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"x"}}"#;
        let r = transform_card_body(body, &Variant::V1JsonRpc, &deny_all_rules(),
                                    Surface::Extended, &Identity::default(), &mut |_| {});
        assert!(matches!(r, Ok(None))); // None => pass through untouched
    }

    #[test]
    fn non_card_passes_through() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{"foo":"bar"}}"#;
        let r = transform_card_body(body, &Variant::V1JsonRpc, &deny_all_rules(),
                                    Surface::Extended, &Identity::default(), &mut |_| {});
        assert!(matches!(r, Ok(None)));
    }

    #[test]
    fn jsonrpc_result_card_governed_and_rewrapped() {
        let body = br#"{"jsonrpc":"2.0","id":7,"result":{"name":"A","skills":[{"id":"a"}]}}"#;
        let out = transform_card_body(body, &Variant::V1JsonRpc, &deny_all_rules(),
                                      Surface::Extended, &Identity::default(), &mut |_| {}).unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["id"], 7);                                   // envelope preserved
        assert_eq!(v["result"]["skills"].as_array().unwrap().len(), 0);
    }
}
```

Note the signature the tests pin down: `transform_card_body(body: &[u8], variant: &Variant, rules: &GovernorRules, surface: Surface, id: &Identity, warn: &mut dyn FnMut(String)) -> anyhow::Result<Option<Vec<u8>>>` — `Ok(None)` = pass through untouched, `Ok(Some(bytes))` = new body, `Err` = fail-closed.

- [ ] **Step 2: Run to verify fail** — `make test` → FAIL (`transform_card_body` undefined).

- [ ] **Step 3: Implement `transform_card_body` + `response_filter`**

```rust
use crate::a2a::{is_agent_card, Skill};
use serde_json::Value;

/// Pure card transform. Returns Ok(None) to pass through, Ok(Some(bytes)) to
/// replace the body, Err to fail closed.
fn transform_card_body(
    body: &[u8], variant: &Variant, rules: &GovernorRules,
    surface: Surface, id: &Identity, warn: &mut dyn FnMut(String),
) -> Result<Option<Vec<u8>>> {
    let root: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Ok(None), // not JSON → not our target
    };
    let is_rpc = matches!(variant, Variant::Legacy | Variant::V1JsonRpc);
    // Locate the card object (borrow immutably first to decide).
    let card_ref = if is_rpc {
        if root.get("error").is_some() { return Ok(None); }      // upstream error → pass through
        match root.get("result") { Some(r) => r, None => return Ok(None) }
    } else { &root };
    if !is_agent_card(card_ref) { return Ok(None); }             // confirmed non-card → pass through

    // From here the body IS a card: any failure is fail-closed (Err).
    let mut root = root; // take ownership to mutate
    let card = if is_rpc { root.get_mut("result").unwrap() } else { &mut root };
    let skills_val = card.get_mut("skills").ok_or_else(|| anyhow!("card lost skills[]"))?;
    let skills: Vec<Skill> = serde_json::from_value(skills_val.take())
        .map_err(|e| anyhow!("skills[] not a skill array: {}", e))?;
    let governed = rules.govern(skills, surface, id, warn);
    *skills_val = serde_json::to_value(&governed).map_err(|e| anyhow!("serialize skills: {}", e))?;
    let out = serde_json::to_vec(&root).map_err(|e| anyhow!("serialize card: {}", e))?;
    Ok(Some(out))
}

async fn response_filter(
    response_state: ResponseState,
    request_data: RequestData<Option<CardContext>>,
    rules: &GovernorRules,
) {
    let ctx = match request_data {
        RequestData::Continue(Some(ctx)) => ctx,
        _ => return, // not a card fetch → pass through
    };
    let headers = response_state.into_headers_state().await;
    let body_state = headers.into_body_state().await;
    let handler = body_state.handler();
    let body = handler.body();

    let mut warned = |m: String| pdk::logger::warn!("[skill-governor] {}", m);
    match transform_card_body(&body, &ctx.variant, rules, ctx.surface, &ctx.identity, &mut warned) {
        Ok(None) => {}                                   // pass through
        Ok(Some(new_body)) => set_response_body(handler, &new_body), // see pdk-request-headers-bodies
        Err(e) => {
            pdk::logger::error!("[skill-governor] failing closed on card shaping: {}", e);
            send_fail_closed(handler, &ctx.variant);     // surface-aware error (spec §7)
        }
    }
}
```

Implement `set_response_body` (write bytes + update `content-length`; **pdk-request-headers-bodies** has the exact API) and `send_fail_closed` (build the surface-appropriate error body: JSON-RPC `-32603` at HTTP 200 for RPC variants, `google.rpc.Status` HTTP 500 for `V1HttpJson`, plain HTTP 500 for `PublicGet` — **pdk-a2a** error envelopes + **pdk-stop-execution** for replacing the response).

- [ ] **Step 4: Run tests** — `make test`, all `resp_tests` PASS.

- [ ] **Step 5: Build** — `make build` PASS.

- [ ] **Step 6: Commit**

```bash
git add a2a-agent-card-skill-governor-flex/src/lib.rs
git commit -m "feat: govern skills[] on the response and fail closed on card errors"
```

---

### Task 7: Integration tests (Docker Flex, all surfaces)

**Files:**
- Create: `a2a-agent-card-skill-governor-flex/tests/requests.rs`
- Verify/adjust: `a2a-agent-card-skill-governor-flex/playground/` config so `make run` serves a mock upstream returning an AgentCard.

**Interfaces:** none (black-box HTTP assertions).

> Follow the **pdk-integration-tests** skill: `FlexConfig`, `ApiConfig`, `HttpMockConfig` mocking an upstream that returns a fixed AgentCard on the card paths. Assert against the governed response.

- [ ] **Step 1: Write integration tests**

Cover, each as its own `#[pdk_test]`:
1. **Public well-known** — mock upstream returns a 3-skill card on `GET /.well-known/agent-card.json`; policy configured to `deny any skillId=secret`; assert the response card omits `secret`, keeps the others.
2. **Public ignores identity rule** — rule `deny client=acme skillId=a`; GET well-known with no auth; assert `a` still present.
3. **Extended JSON-RPC (V1)** — POST `GetExtendedAgentCard` with `A2A-Version: 1.0`; upstream returns card in `result`; assert `result.skills` governed and JSON-RPC `id` preserved.
4. **Extended HTTP+JSON** — `GET /extendedAgentCard` + `A2A-Version: 1.0`; assert governed bare-card body.
5. **Inject** — `skills` upsert with a new id + name + description; assert it appears appended.
6. **Rewrite** — upsert existing id with a new `description`; assert overwritten, other fields intact.
7. **Non-card passthrough** — `message/send` response flows through byte-identical.
8. **Empty ruleset passthrough** — no rules; card returned byte-identical.

```rust
// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
// Integration tests — see pdk-integration-tests skill for FlexConfig/ApiConfig/HttpMockConfig setup.
// (Full harness scaffolding per the skill; assertions per the list above.)
```

- [ ] **Step 2: Run** — `make test` (integration profile). Expected: all pass. If Docker/registration is unavailable, document the blocker and run the unit suite; do not mark the task complete on a skipped integration run without saying so.

- [ ] **Step 3: Commit**

```bash
git add a2a-agent-card-skill-governor-flex/tests/requests.rs a2a-agent-card-skill-governor-flex/playground
git commit -m "test: integration coverage across all three card surfaces"
```

---

### Task 8: Docs alignment + policy testing SKILL

**Files:**
- Create/Modify: `a2a-agent-card-skill-governor/README.md`, `a2a-agent-card-skill-governor-definition/README.md`, `a2a-agent-card-skill-governor-flex/README.md`
- Create: `a2a-agent-card-skill-governor-flex/.claude/test-a2a-agent-card-skill-governor-locally/SKILL.md` (per pdk-policy-tester convention)
- Verify: spec doc still matches final behavior; update if any decision drifted during implementation.

**Interfaces:** none.

- [ ] **Step 1: Write the root README** — what the policy does, the two surfaces, config reference (every gcl property with an example rule for allow/deny/rewrite/inject), the disclosure≠authorization caveat, and the fail-closed behavior. Keep the Exchange `description` label ≤256 chars; long prose lives here.

- [ ] **Step 2: Write the local-testing SKILL.md** — `make run` playground steps + `curl` examples hitting all three card surfaces with/without `A2A-Version: 1.0`.

- [ ] **Step 3: Reconcile the spec** — re-read `docs/superpowers/specs/2026-07-02-*.md`; if implementation diverged (field renames, extra WARN cases), update the spec so docs and code agree.

- [ ] **Step 4: Commit**

```bash
git add a2a-agent-card-skill-governor/README.md a2a-agent-card-skill-governor-definition/README.md a2a-agent-card-skill-governor-flex/README.md a2a-agent-card-skill-governor-flex/.claude docs
git commit -m "docs: README, local-testing skill, and spec reconciliation"
```

---

## Self-review notes

- **Spec coverage:** §3 surfaces → Task 5 detect + Task 7 tests 1/3/4; §5 schema → Task 2; §6 engine (visibility first-match, upsert, default-allow, audience no-op on public, wholesale array replace, inject validation, deny-reinject WARN, glob) → Task 4 tests; §7 failure modes (non-card passthrough, JSON-RPC error passthrough, fail-closed) → Task 6 tests + `send_fail_closed`; identity from `Authentication` only → Task 5; empty-ruleset passthrough → Task 4 + Task 7 test 8.
- **Deferred/out-of-scope (spec §9):** gRPC binding, invocation-time authz, additive array merge, direct JWT parsing, metrics counters — intentionally not tasked.
- **PDK-API gaps to resolve against skills during implementation** (not placeholders — named APIs with a skill pointer): `Authentication` reads (pdk-authentication), `A2A-Version`/JSON-RPC parse (pdk-a2a), `:path` + body read/write + content-length (pdk-request-headers-bodies), response replacement (pdk-stop-execution), SLA tier if used (pdk-metadata), integration harness (pdk-integration-tests). Generated config field names are authoritative over Task 2's illustrative struct.
