// Copyright 2026 Salesforce, Inc. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Rule-evaluation engine. Pure — no PDK deps, fully unit-testable.
//! Model C: first-match visibility gate, then layered skill upsert.

use crate::a2a::Skill;
use crate::generated::config::{Config, SkillPayload, SkillRule, VisibilityRule};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Public,
    Extended,
}

#[derive(Default, Clone)]
pub struct Identity {
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    pub tier: Option<String>,
    pub scopes: Vec<String>,
}

enum Audience {
    Any,
    Client(String),
    Scope(String),
    Tier(String),
}

enum Target {
    All,
    Exact(String),
    Glob(String),
}

struct CompiledVis {
    effect_allow: bool,
    audience: Audience,
    target: Target,
}
struct CompiledUpsert {
    audience: Audience,
    skill: SkillPayload,
}

pub struct GovernorRules {
    default_allow: bool,
    visibility: Vec<CompiledVis>,
    upserts: Vec<CompiledUpsert>,
}

fn compile_audience(
    at: Option<&str>,
    av: Option<&str>,
    warn: &mut dyn FnMut(String),
) -> Option<Audience> {
    match at.unwrap_or("any") {
        "any" => Some(Audience::Any),
        kind @ ("client" | "scope" | "tier") => match av {
            Some(v) if !v.is_empty() => Some(match kind {
                "client" => Audience::Client(v.into()),
                "scope" => Audience::Scope(v.into()),
                _ => Audience::Tier(v.into()),
            }),
            _ => {
                warn(format!(
                    "rule with audienceType '{kind}' missing audienceValue — dropped"
                ));
                None
            }
        },
        other => {
            warn(format!("unknown audienceType '{other}' — dropped"));
            None
        }
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
            Audience::Client(v) => {
                id.client_id.as_deref() == Some(v) || id.client_name.as_deref() == Some(v)
            }
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
            let VisibilityRule {
                effect,
                audience_type,
                audience_value,
                skill_id,
                skill_id_pattern,
            } = r;
            let effect_allow = match effect.as_str() {
                "allow" => true,
                "deny" => false,
                other => {
                    warn(format!("unknown visibility effect '{other}' — dropped"));
                    continue;
                }
            };
            let Some(audience) =
                compile_audience(audience_type.as_deref(), audience_value.as_deref(), warn)
            else {
                continue;
            };
            visibility.push(CompiledVis {
                effect_allow,
                audience,
                target: compile_target(skill_id.as_deref(), skill_id_pattern.as_deref()),
            });
        }
        let mut upserts = Vec::new();
        for r in config.skills.iter().flatten() {
            let SkillRule {
                audience_type,
                audience_value,
                skill,
            } = r;
            if skill.id.is_empty() {
                warn("skill upsert with empty id — dropped".into());
                continue;
            }
            let Some(audience) =
                compile_audience(audience_type.as_deref(), audience_value.as_deref(), warn)
            else {
                continue;
            };
            upserts.push(CompiledUpsert {
                audience,
                skill: skill.clone(),
            });
        }
        GovernorRules {
            default_allow,
            visibility,
            upserts,
        }
    }

    pub fn govern(
        &self,
        skills: Vec<Skill>,
        surface: Surface,
        id: &Identity,
        warn: &mut dyn FnMut(String),
    ) -> Vec<Skill> {
        // 1. Visibility gate — first-match per skill.
        let mut denied_ids: Vec<String> = Vec::new();
        let mut survivors: Vec<Skill> = skills
            .into_iter()
            .filter(|s| {
                let mut verdict = self.default_allow;
                for r in &self.visibility {
                    if r.audience.matches(surface, id) && r.target.matches(&s.id) {
                        verdict = r.effect_allow;
                        break;
                    }
                }
                if !verdict {
                    denied_ids.push(s.id.clone());
                }
                verdict
            })
            .collect();

        // 2. Skill upsert — layered, declaration order.
        for u in &self.upserts {
            if !u.audience.matches(surface, id) {
                continue;
            }
            match survivors.iter_mut().find(|s| s.id == u.skill.id) {
                Some(existing) => apply_rewrite(existing, &u.skill),
                None => {
                    if u.skill.name.is_some() && u.skill.description.is_some() {
                        if denied_ids.iter().any(|d| d == &u.skill.id) {
                            warn(format!(
                                "skill '{}' was denied then re-injected",
                                u.skill.id
                            ));
                        }
                        survivors.push(payload_to_skill(&u.skill));
                    } else {
                        warn(format!(
                            "inject '{}' missing name/description — skipped",
                            u.skill.id
                        ));
                    }
                }
            }
        }
        survivors
    }
}

fn apply_rewrite(dst: &mut Skill, src: &SkillPayload) {
    if let Some(v) = &src.name {
        dst.name = Some(v.clone());
    }
    if let Some(v) = &src.description {
        dst.description = Some(v.clone());
    }
    if let Some(v) = &src.tags {
        dst.tags = Some(v.clone());
    }
    if let Some(v) = &src.examples {
        dst.examples = Some(v.clone());
    }
    if let Some(v) = &src.input_modes {
        dst.input_modes = Some(v.clone());
    }
    if let Some(v) = &src.output_modes {
        dst.output_modes = Some(v.clone());
    }
}

fn payload_to_skill(p: &SkillPayload) -> Skill {
    Skill {
        id: p.id.clone(),
        name: p.name.clone(),
        description: p.description.clone(),
        tags: p.tags.clone(),
        examples: p.examples.clone(),
        input_modes: p.input_modes.clone(),
        output_modes: p.output_modes.clone(),
        extra: serde_json::Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::Skill;
    use crate::generated::config::*;
    use serde_json::Map;

    fn skill(id: &str) -> Skill {
        Skill {
            id: id.into(),
            name: Some(id.into()),
            description: None,
            tags: None,
            examples: None,
            input_modes: None,
            output_modes: None,
            extra: Map::new(),
        }
    }
    fn cfg(vis: Vec<VisibilityRule>, sk: Vec<SkillRule>, default_allow: bool) -> Config {
        Config {
            scope_claim_key: None,
            default_allow: Some(default_allow),
            visibility: Some(vis),
            skills: Some(sk),
        }
    }
    fn vis(effect: &str, at: &str, av: Option<&str>, sid: Option<&str>) -> VisibilityRule {
        VisibilityRule {
            effect: effect.into(),
            audience_type: Some(at.into()),
            audience_value: av.map(Into::into),
            skill_id: sid.map(Into::into),
            skill_id_pattern: None,
        }
    }
    fn nowarn() -> impl FnMut(String) {
        |_| {}
    }
    fn anon() -> Identity {
        Identity::default()
    }

    #[test]
    fn empty_ruleset_is_full_passthrough() {
        let c = Config {
            scope_claim_key: None,
            default_allow: Some(true),
            visibility: Some(vec![]),
            skills: Some(vec![]),
        };
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(
            vec![skill("a"), skill("b")],
            Surface::Public,
            &anon(),
            &mut w,
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn deny_removes_skill_first_match_wins() {
        let c = cfg(
            vec![
                vis("deny", "any", None, Some("a")),
                vis("allow", "any", None, Some("a")),
            ],
            vec![],
            true,
        );
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(
            vec![skill("a"), skill("b")],
            Surface::Extended,
            &anon(),
            &mut w,
        );
        assert_eq!(
            out.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            vec!["b"]
        );
    }

    #[test]
    fn default_deny_hides_unmatched() {
        let c = cfg(vec![vis("allow", "any", None, Some("a"))], vec![], false);
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(
            vec![skill("a"), skill("b")],
            Surface::Extended,
            &anon(),
            &mut w,
        );
        assert_eq!(
            out.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            vec!["a"]
        );
    }

    #[test]
    fn identity_rule_noops_on_public_surface() {
        // deny client=acme on a: must NOT apply on the public (unauthenticated) card
        let c = cfg(
            vec![vis("deny", "client", Some("acme"), Some("a"))],
            vec![],
            true,
        );
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("a")], Surface::Public, &anon(), &mut w);
        assert_eq!(out.len(), 1); // survived — identity rule ignored on public
    }

    #[test]
    fn scope_rule_matches_on_extended() {
        let c = cfg(
            vec![vis("deny", "scope", Some("admin"), Some("a"))],
            vec![],
            true,
        );
        let id = Identity {
            scopes: vec!["admin".into()],
            ..Default::default()
        };
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("a")], Surface::Extended, &id, &mut w);
        assert!(out.is_empty());
    }

    #[test]
    fn upsert_rewrites_existing_wholesale_arrays() {
        let sr = SkillRule {
            audience_type: Some("any".into()),
            audience_value: None,
            skill: SkillPayload {
                id: "a".into(),
                name: Some("New".into()),
                description: None,
                tags: Some(vec!["x".into()]),
                examples: None,
                input_modes: None,
                output_modes: None,
            },
        };
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
        let sr = SkillRule {
            audience_type: Some("any".into()),
            audience_value: None,
            skill: SkillPayload {
                id: "new".into(),
                name: Some("N".into()),
                description: Some("D".into()),
                tags: None,
                examples: None,
                input_modes: None,
                output_modes: None,
            },
        };
        let c = cfg(vec![], vec![sr], true);
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(vec![skill("a")], Surface::Extended, &anon(), &mut w);
        assert_eq!(
            out.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            vec!["a", "new"]
        );
    }

    #[test]
    fn inject_missing_name_or_description_warns_and_skips() {
        let sr = SkillRule {
            audience_type: Some("any".into()),
            audience_value: None,
            skill: SkillPayload {
                id: "bad".into(),
                name: None,
                description: None,
                tags: None,
                examples: None,
                input_modes: None,
                output_modes: None,
            },
        };
        let c = cfg(vec![], vec![sr], true);
        let mut warnings = Vec::new();
        let g = GovernorRules::compile(&c, &mut |m| warnings.push(m));
        let out = g.govern(vec![skill("a")], Surface::Extended, &anon(), &mut |m| {
            warnings.push(m)
        });
        assert_eq!(out.len(), 1); // "bad" not injected
        assert!(warnings
            .iter()
            .any(|m| m.contains("bad") && m.contains("name")));
    }

    #[test]
    fn glob_pattern_denies_matching_skills() {
        let mut r = vis("deny", "any", None, None);
        r.skill_id_pattern = Some("admin.*".into());
        let c = cfg(vec![r], vec![], true);
        let mut w = nowarn();
        let g = GovernorRules::compile(&c, &mut w);
        let out = g.govern(
            vec![skill("admin.reset"), skill("user.read")],
            Surface::Extended,
            &anon(),
            &mut w,
        );
        assert_eq!(
            out.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            vec!["user.read"]
        );
    }
}
