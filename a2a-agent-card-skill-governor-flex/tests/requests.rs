// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
// Integration tests — see pdk-integration-tests skill for FlexConfig/ApiConfig/HttpMockConfig setup.
//
// Black-box HTTP coverage of the A2A Agent Card Skill Governor across all three
// card surfaces:
//   - Public well-known:     GET /.well-known/agent-card.json (unauthenticated)
//   - Extended JSON-RPC:     POST GetExtendedAgentCard + A2A-Version: 1.0
//   - Extended HTTP+JSON:    GET /extendedAgentCard + A2A-Version: 1.0
//
// A mock upstream returns a fixed AgentCard on the card paths; assertions run on
// the governed response that reaches the client. All tests live in this single
// binary to keep the test-build time down.

mod common;

use common::setup::*;
use pdk_test::pdk_test;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Shared config helpers
// ---------------------------------------------------------------------------

/// Deny the `secret` skill for everyone (audienceType any).
fn deny_secret_cfg() -> Value {
    json!({
        "visibility": [
            { "effect": "deny", "audienceType": "any", "skillId": "secret" }
        ]
    })
}

/// Collect the skill ids from a bare card body.
fn skill_ids(card: &Value) -> Vec<String> {
    card["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect()
}

// ===========================================================================
// Case 1 — Public well-known: deny any skillId=secret; secret omitted, rest kept
// ===========================================================================

#[pdk_test]
async fn public_well_known_denies_secret_skill() -> anyhow::Result<()> {
    let (_c, url, mock) = setup_test(deny_secret_cfg()).await?;
    mock_well_known(&mock, three_skill_card()).await;

    let resp = get_well_known(&url).await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    let ids = skill_ids(&body);
    assert!(!ids.contains(&"secret".to_string()), "secret must be denied");
    assert!(ids.contains(&"search".to_string()));
    assert!(ids.contains(&"report".to_string()));
    Ok(())
}

// ===========================================================================
// Case 2 — Public ignores identity rule: deny client=acme skillId=a; a stays
// ===========================================================================

#[pdk_test]
async fn public_well_known_ignores_identity_rule() -> anyhow::Result<()> {
    let cfg = json!({
        "visibility": [
            { "effect": "deny", "audienceType": "client", "audienceValue": "acme", "skillId": "a" }
        ]
    });
    let (_c, url, mock) = setup_test(cfg).await?;
    mock_well_known(&mock, agent_card(vec![skill("a", "A"), skill("b", "B")])).await;

    let resp = get_well_known(&url).await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    let ids = skill_ids(&body);
    // Identity-scoped rules never bind on the unauthenticated public card.
    assert!(ids.contains(&"a".to_string()), "identity rule must no-op on public");
    assert!(ids.contains(&"b".to_string()));
    Ok(())
}

// ===========================================================================
// Case 3 — Extended JSON-RPC (V1): result.skills governed, id preserved
// ===========================================================================

#[pdk_test]
async fn extended_jsonrpc_governs_result_and_preserves_id() -> anyhow::Result<()> {
    let (_c, url, mock) = setup_test(deny_secret_cfg()).await?;
    mock_ext_jsonrpc(&mock, json!("rpc-77"), three_skill_card()).await;

    let resp = post_ext_jsonrpc(&url, json!("rpc-77")).await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], "rpc-77", "JSON-RPC id must be preserved");
    let ids = skill_ids(&body["result"]);
    assert!(!ids.contains(&"secret".to_string()));
    assert!(ids.contains(&"search".to_string()));
    assert!(ids.contains(&"report".to_string()));
    Ok(())
}

// ===========================================================================
// Case 4 — Extended HTTP+JSON: GET /extendedAgentCard governs the bare card
// ===========================================================================

#[pdk_test]
async fn extended_httpjson_governs_bare_card() -> anyhow::Result<()> {
    let (_c, url, mock) = setup_test(deny_secret_cfg()).await?;
    mock_ext_httpjson(&mock, three_skill_card()).await;

    let resp = get_ext_httpjson(&url).await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    // Bare card (no JSON-RPC envelope).
    assert!(body.get("result").is_none());
    let ids = skill_ids(&body);
    assert!(!ids.contains(&"secret".to_string()));
    assert!(ids.contains(&"search".to_string()));
    Ok(())
}

// ===========================================================================
// Case 5 — Inject: upsert a new skill with id+name+description → appended
// ===========================================================================

#[pdk_test]
async fn inject_new_skill_is_appended() -> anyhow::Result<()> {
    let cfg = json!({
        "skills": [
            {
                "audienceType": "any",
                "skill": {
                    "id": "injected",
                    "name": "Injected Skill",
                    "description": "Added by the governor"
                }
            }
        ]
    });
    let (_c, url, mock) = setup_test(cfg).await?;
    mock_well_known(&mock, agent_card(vec![skill("search", "Search")])).await;

    let resp = get_well_known(&url).await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    let arr = body["skills"].as_array().unwrap();
    let ids = skill_ids(&body);
    assert_eq!(ids, vec!["search", "injected"], "injected skill appended after existing");
    // Confirm the injected fields landed.
    let injected = arr.iter().find(|s| s["id"] == "injected").unwrap();
    assert_eq!(injected["name"], "Injected Skill");
    assert_eq!(injected["description"], "Added by the governor");
    Ok(())
}

// ===========================================================================
// Case 6 — Rewrite: upsert existing id with new description; other fields intact
// ===========================================================================

#[pdk_test]
async fn rewrite_existing_skill_description() -> anyhow::Result<()> {
    let cfg = json!({
        "skills": [
            {
                "audienceType": "any",
                "skill": { "id": "search", "description": "Governed description" }
            }
        ]
    });
    let (_c, url, mock) = setup_test(cfg).await?;
    // Card skill has name + tags that must survive the description rewrite.
    let card = agent_card(vec![json!({
        "id": "search",
        "name": "Original Search",
        "description": "Original description",
        "tags": ["a", "b"]
    })]);
    mock_well_known(&mock, card).await;

    let resp = get_well_known(&url).await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    let s = &body["skills"][0];
    assert_eq!(s["description"], "Governed description", "description overwritten");
    assert_eq!(s["name"], "Original Search", "name intact");
    assert_eq!(s["tags"], json!(["a", "b"]), "tags intact");
    Ok(())
}

// ===========================================================================
// Case 7 — Non-card passthrough: message/send response flows byte-identical
// ===========================================================================

#[pdk_test]
async fn non_card_message_send_passes_through_byte_identical() -> anyhow::Result<()> {
    let (_c, url, mock) = setup_test(deny_secret_cfg()).await?;
    // A message/send result is not an AgentCard (no skills[] array).
    let upstream = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "result": { "status": { "state": "completed" }, "artifacts": [] }
    });
    mock_jsonrpc_body(&mock, upstream.clone()).await;

    let resp = post_jsonrpc(
        &url,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "message/send", "params": {} }),
    )
    .await?;
    assert_eq!(resp.status(), 200);
    let raw = resp.text().await?;
    let got: Value = serde_json::from_str(&raw)?;
    assert_eq!(got, upstream, "non-card response must be untouched");
    Ok(())
}

// ===========================================================================
// Case 8 — Empty ruleset passthrough: card returned byte-identical
// ===========================================================================

#[pdk_test]
async fn empty_ruleset_returns_card_untouched() -> anyhow::Result<()> {
    let (_c, url, mock) = setup_test(json!({})).await?;
    let card = three_skill_card();
    mock_well_known(&mock, card.clone()).await;

    let resp = get_well_known(&url).await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    // No rules → skills[] identical (order + content).
    assert_eq!(body["skills"], card["skills"]);
    assert_eq!(skill_ids(&body), vec!["search", "secret", "report"]);
    Ok(())
}

// ===========================================================================
// Carried Critical C-1 — fail-closed on a confirmed card that cannot be shaped.
//
// The upstream returns a body whose `skills` IS an array (so `is_agent_card`
// is true and the transform commits to shaping it) but whose entry has a
// numeric `id`, which fails `Vec<Skill>` deserialization (Skill.id: String).
// The policy MUST fail closed rather than ship an ungoverned card.
//
//   Public well-known  → HTTP 500
//   Extended HTTP+JSON → HTTP 500
//   Extended JSON-RPC  → HTTP 200 + JSON-RPC error.code == -32603
//
// This test is the authority on whether Envoy honours the `:status` rewrite
// the policy issues from the combined headers-body state. If the two HTTP-500
// assertions fail, that is a Task-6 defect (send_fail_closed cannot rewrite
// status from ResponseHeadersBodyState) — the assertions stay spec-correct so
// the defect stays visible.
// ===========================================================================

/// A body that passes `is_agent_card` (skills is an array) but fails skill
/// deserialization (id must be a String, here it is a number).
fn malformed_skill_card() -> Value {
    json!({
        "name": "Broken Agent",
        "skills": [ { "id": 123, "name": "bad" } ]
    })
}

#[pdk_test]
async fn fail_closed_public_well_known_returns_500() -> anyhow::Result<()> {
    let (_c, url, mock) = setup_test(deny_secret_cfg()).await?;
    mock_well_known(&mock, malformed_skill_card()).await;

    let resp = get_well_known(&url).await?;
    assert_eq!(
        resp.status(),
        500,
        "public well-known must fail closed with HTTP 500"
    );
    Ok(())
}

#[pdk_test]
async fn fail_closed_extended_httpjson_returns_500() -> anyhow::Result<()> {
    let (_c, url, mock) = setup_test(deny_secret_cfg()).await?;
    mock_ext_httpjson(&mock, malformed_skill_card()).await;

    let resp = get_ext_httpjson(&url).await?;
    assert_eq!(
        resp.status(),
        500,
        "extended HTTP+JSON must fail closed with HTTP 500"
    );
    Ok(())
}

#[pdk_test]
async fn fail_closed_extended_jsonrpc_returns_internal_error() -> anyhow::Result<()> {
    let (_c, url, mock) = setup_test(deny_secret_cfg()).await?;
    // JSON-RPC result wrapping the malformed card.
    mock_ext_jsonrpc(&mock, json!("rpc-err"), malformed_skill_card()).await;

    let resp = post_ext_jsonrpc(&url, json!("rpc-err")).await?;
    // JSON-RPC in-band convention: HTTP stays 200, error is in the body.
    assert_eq!(resp.status(), 200, "JSON-RPC fail-closed stays HTTP 200");
    let body: Value = resp.json().await?;
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(
        body["error"]["code"], -32603,
        "JSON-RPC internal-error code on fail-closed"
    );
    assert!(body.get("result").is_none() || body["result"].is_null());
    Ok(())
}

// ===========================================================================
// Carried tier-key confirmation — a tier-audience rule driven over a live
// request. Confirms whether the `Authentication` injectable in the integration
// harness carries an SLA tier the policy can read (probed keys:
// ["tier","sla_tier","slaTier"]).
//
// The pdk-integration-tests harness (FlexConfig/ApiConfig) applies only THIS
// policy — there is no upstream SLA-tier / client-credential policy in the
// chain, and no client application contract, so nothing populates the
// `Authentication` injectable's custom properties with a tier. This test
// therefore documents the harness limitation rather than confirming the key
// spelling: with no tier present, the tier rule degrades to no-op and the skill
// survives (default_allow = true). We assert that graceful-degradation
// behaviour — NOT a synthesized/faked tier. See task-7-report.md for the
// tier-key confirmation outcome.
// ===========================================================================

#[pdk_test]
async fn tier_rule_degrades_gracefully_without_injected_tier() -> anyhow::Result<()> {
    let cfg = json!({
        "visibility": [
            { "effect": "deny", "audienceType": "tier", "audienceValue": "gold", "skillId": "search" }
        ]
    });
    let (_c, url, mock) = setup_test(cfg).await?;
    // Extended surface (identity rules only bind here), but no auth policy in
    // the chain means Identity.tier is None → the tier rule cannot match.
    mock_ext_jsonrpc(&mock, json!(1), agent_card(vec![skill("search", "Search")])).await;

    let resp = post_ext_jsonrpc(&url, json!(1)).await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    let ids = skill_ids(&body["result"]);
    // Tier unmatched → default_allow keeps the skill. This proves the multi-key
    // probe degrades to None safely; it does NOT confirm the tier-key spelling.
    assert!(
        ids.contains(&"search".to_string()),
        "tier rule must no-op when no tier is present in Authentication"
    );
    Ok(())
}
