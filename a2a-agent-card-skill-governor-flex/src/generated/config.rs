// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(alias = "scopeClaimKey")]
    pub scope_claim_key: Option<String>,
    #[serde(alias = "defaultAllow")]
    pub default_allow: Option<bool>,
    #[serde(alias = "visibility")]
    pub visibility: Option<Vec<VisibilityRule>>,
    #[serde(alias = "skills")]
    pub skills: Option<Vec<SkillRule>>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct VisibilityRule {
    #[serde(alias = "effect")]
    pub effect: String,
    #[serde(alias = "audienceType")]
    pub audience_type: Option<String>,
    #[serde(alias = "audienceValue")]
    pub audience_value: Option<String>,
    #[serde(alias = "skillId")]
    pub skill_id: Option<String>,
    #[serde(alias = "skillIdPattern")]
    pub skill_id_pattern: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SkillRule {
    #[serde(alias = "audienceType")]
    pub audience_type: Option<String>,
    #[serde(alias = "audienceValue")]
    pub audience_value: Option<String>,
    #[serde(alias = "skill")]
    pub skill: SkillPayload,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SkillPayload {
    #[serde(alias = "id")]
    pub id: String,
    #[serde(alias = "name")]
    pub name: Option<String>,
    #[serde(alias = "description")]
    pub description: Option<String>,
    #[serde(alias = "tags")]
    pub tags: Option<Vec<String>>,
    #[serde(alias = "examples")]
    pub examples: Option<Vec<String>>,
    #[serde(alias = "inputModes")]
    pub input_modes: Option<Vec<String>>,
    #[serde(alias = "outputModes")]
    pub output_modes: Option<Vec<String>>,
}
