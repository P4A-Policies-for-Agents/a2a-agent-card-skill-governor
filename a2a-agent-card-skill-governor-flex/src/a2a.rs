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
