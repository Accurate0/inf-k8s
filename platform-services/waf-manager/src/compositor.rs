use crate::crd::{WafBlock, WafPolicy};
use ipnet::IpNet;
use kube::ResourceExt;
use serde_json::{Map, Value, json};

/// Name of the generated Deny rule inside the compiled policy.
pub const BLOCK_RULE_NAME: &str = "waf-manager-blocklist";

/// Fields waf-manager owns; a template setting one of these is a conflict.
const RESERVED: &[&str] = &["targetRef", "targetRefs", "targetSelectors", "mergeType"];

/// Fields that can only meaningfully be set once per policy.
const SINGLE_VALUED: &[&str] = &["oidc", "jwt", "basicAuth", "cors", "extAuth", "apiKeyAuth"];

#[derive(Debug, Clone)]
pub struct Compiled {
    /// `None` when nothing contributes and the policy should be deleted.
    pub spec: Option<Value>,

    /// Problems, each naming the WafPolicy responsible.
    pub conflicts: Vec<Conflict>,
}

#[derive(Debug, Clone)]
pub struct Conflict {
    pub source: String,
    pub message: String,
}

/// Folds every WafPolicy and WafBlock for one gateway into one SecurityPolicy spec.
pub struct Compositor {
    gateway: String,
}

impl Compositor {
    pub fn new(gateway: impl Into<String>) -> Self {
        Self {
            gateway: gateway.into(),
        }
    }

    pub fn compile(&self, policies: &[WafPolicy], blocked: &[IpNet]) -> Compiled {
        let mut sorted: Vec<&WafPolicy> = policies.iter().collect();
        sorted.sort_by(|a, b| {
            a.spec
                .priority
                .cmp(&b.spec.priority)
                .then_with(|| a.name_any().cmp(&b.name_any()))
        });

        let mut spec = Map::new();
        let mut rules: Vec<Value> = Vec::new();
        let mut conflicts: Vec<Conflict> = Vec::new();
        let mut default_action: Option<(String, String)> = None;
        let mut owners: Map<String, Value> = Map::new();

        // Denies go first: Envoy evaluates authorization rules in order and takes
        // the first match, so a block placed after a broad Allow would never fire.
        if !blocked.is_empty() {
            rules.push(Self::block_rule(blocked));
        }

        for policy in sorted {
            let source = policy.name_any();
            let Some(template) = policy.spec.template.as_object() else {
                if !policy.spec.template.is_null() {
                    conflicts.push(Conflict {
                        source,
                        message: "template must be an object".to_string(),
                    });
                }
                continue;
            };

            for (key, value) in template {
                if RESERVED.contains(&key.as_str()) {
                    conflicts.push(Conflict {
                        source: source.clone(),
                        message: format!("{key} is managed by waf-manager and was ignored"),
                    });
                    continue;
                }

                if key == "authorization" {
                    self.merge_authorization(
                        value,
                        &source,
                        &mut rules,
                        &mut default_action,
                        &mut conflicts,
                    );
                    continue;
                }

                if SINGLE_VALUED.contains(&key.as_str())
                    && let Some(previous) = owners.get(key)
                {
                    conflicts.push(Conflict {
                        source: source.clone(),
                        message: format!(
                            "{key} is already set by WafPolicy {previous} and was ignored"
                        ),
                    });
                    continue;
                }

                owners.insert(key.clone(), json!(source));
                spec.insert(key.clone(), value.clone());
            }
        }

        if spec.is_empty() && rules.is_empty() && default_action.is_none() {
            return Compiled {
                spec: None,
                conflicts,
            };
        }

        spec.insert(
            "targetRefs".to_string(),
            json!([{
                "group": "gateway.networking.k8s.io",
                "kind": "Gateway",
                "name": self.gateway,
            }]),
        );

        if !rules.is_empty() || default_action.is_some() {
            // Always explicit: the CRD reads an absent value as Deny, which would
            // black-hole every request the rules do not allow.
            let action = default_action
                .map(|(action, _)| action)
                .unwrap_or_else(|| "Allow".to_string());

            spec.insert(
                "authorization".to_string(),
                json!({ "defaultAction": action, "rules": rules }),
            );
        }

        Compiled {
            spec: Some(Value::Object(spec)),
            conflicts,
        }
    }

    fn merge_authorization(
        &self,
        value: &Value,
        source: &str,
        rules: &mut Vec<Value>,
        default_action: &mut Option<(String, String)>,
        conflicts: &mut Vec<Conflict>,
    ) {
        let Some(authorization) = value.as_object() else {
            conflicts.push(Conflict {
                source: source.to_string(),
                message: "authorization must be an object".to_string(),
            });
            return;
        };

        if let Some(action) = authorization.get("defaultAction").and_then(Value::as_str) {
            match default_action {
                Some((existing, owner)) if existing != action => conflicts.push(Conflict {
                    source: source.to_string(),
                    message: format!(
                        "authorization.defaultAction={action} conflicts with {existing} \
                         set by WafPolicy {owner}; kept {existing}"
                    ),
                }),
                Some(_) => {}
                None => *default_action = Some((action.to_string(), source.to_string())),
            }
        }

        let Some(template_rules) = authorization.get("rules") else {
            return;
        };

        let Some(template_rules) = template_rules.as_array() else {
            conflicts.push(Conflict {
                source: source.to_string(),
                message: "authorization.rules must be an array".to_string(),
            });
            return;
        };

        for rule in template_rules {
            if rule.get("name").and_then(Value::as_str) == Some(BLOCK_RULE_NAME) {
                conflicts.push(Conflict {
                    source: source.to_string(),
                    message: format!("rule named {BLOCK_RULE_NAME} is reserved and was ignored"),
                });
                continue;
            }

            rules.push(rule.clone());
        }
    }

    fn block_rule(blocked: &[IpNet]) -> Value {
        let mut cidrs: Vec<String> = blocked.iter().map(|net| net.to_string()).collect();
        cidrs.sort();
        cidrs.dedup();

        json!({
            "name": BLOCK_RULE_NAME,
            "action": "Deny",
            "principal": { "clientCIDRs": cidrs },
        })
    }

    /// Unexpired blocks belonging to this gateway.
    pub fn active_cidrs(
        &self,
        blocks: &[WafBlock],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Vec<IpNet> {
        blocks
            .iter()
            .filter(|block| block.spec.gateway == self.gateway)
            .filter(|block| !block.is_expired(now))
            .filter_map(|block| crate::allowlist::Allowlist::parse_cidr(&block.spec.cidr).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{WafBlockSpec, WafPolicySpec};

    fn policy(name: &str, priority: i32, template: Value) -> WafPolicy {
        let mut p = WafPolicy::new(
            name,
            WafPolicySpec {
                gateway: "public-gateway".to_string(),
                gateway_namespace: "envoy-gateway-system".to_string(),
                priority,
                template,
            },
        );
        p.metadata.namespace = Some("waf-manager".to_string());
        p
    }

    fn block(name: &str, cidr: &str) -> WafBlock {
        WafBlock::new(
            name,
            WafBlockSpec {
                cidr: cidr.to_string(),
                gateway: "public-gateway".to_string(),
                reason: None,
                rule_ids: None,
                expires_at: None,
                created_by: None,
            },
        )
    }

    fn net(s: &str) -> IpNet {
        s.parse().unwrap()
    }

    #[test]
    fn nothing_to_do_compiles_to_no_policy() {
        let compiled = Compositor::new("public-gateway").compile(&[], &[]);
        assert!(compiled.spec.is_none());
        assert!(compiled.conflicts.is_empty());
    }

    #[test]
    fn blocks_alone_produce_a_deny_rule_with_default_allow() {
        let compiled = Compositor::new("public-gateway").compile(&[], &[net("203.0.113.4/32")]);
        let spec = compiled.spec.unwrap();

        assert_eq!(spec["authorization"]["defaultAction"], "Allow");
        let rules = spec["authorization"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["action"], "Deny");
        assert_eq!(rules[0]["principal"]["clientCIDRs"][0], "203.0.113.4/32");
        assert_eq!(spec["targetRefs"][0]["name"], "public-gateway");
    }

    #[test]
    fn block_rule_precedes_template_rules() {
        // A template Allow-all placed first would shadow every deny, since Envoy
        // takes the first matching rule.
        let p = policy(
            "allow-all",
            0,
            json!({ "authorization": { "rules": [
                { "name": "allow-all", "action": "Allow", "principal": { "clientCIDRs": ["0.0.0.0/0"] } }
            ]}}),
        );

        let compiled = Compositor::new("public-gateway").compile(&[p], &[net("203.0.113.4/32")]);
        let spec = compiled.spec.unwrap();
        let rules = spec["authorization"]["rules"].as_array().unwrap();

        assert_eq!(rules[0]["name"], BLOCK_RULE_NAME);
        assert_eq!(rules[1]["name"], "allow-all");
    }

    #[test]
    fn templates_merge_by_priority_then_name() {
        let a = policy(
            "b-second",
            10,
            json!({ "authorization": { "rules": [{ "name": "b" }] } }),
        );
        let b = policy(
            "a-first",
            5,
            json!({ "authorization": { "rules": [{ "name": "a" }] } }),
        );

        let compiled = Compositor::new("public-gateway").compile(&[a, b], &[]);
        let spec = compiled.spec.unwrap();
        let rules = spec["authorization"]["rules"].as_array().unwrap();

        assert_eq!(rules[0]["name"], "a");
        assert_eq!(rules[1]["name"], "b");
    }

    #[test]
    fn oidc_from_one_policy_is_carried_through() {
        let p = policy(
            "auth",
            0,
            json!({ "oidc": { "provider": { "issuer": "https://idm" } } }),
        );
        let compiled = Compositor::new("public-gateway").compile(&[p], &[]);
        let spec = compiled.spec.unwrap();

        assert_eq!(spec["oidc"]["provider"]["issuer"], "https://idm");
    }

    #[test]
    fn second_policy_setting_oidc_is_a_conflict_and_first_wins() {
        let a = policy(
            "a-auth",
            0,
            json!({ "oidc": { "provider": { "issuer": "https://first" } } }),
        );
        let b = policy(
            "b-auth",
            1,
            json!({ "oidc": { "provider": { "issuer": "https://second" } } }),
        );

        let compiled = Compositor::new("public-gateway").compile(&[a, b], &[]);
        let spec = compiled.spec.unwrap();

        assert_eq!(spec["oidc"]["provider"]["issuer"], "https://first");
        assert_eq!(compiled.conflicts.len(), 1);
        assert_eq!(compiled.conflicts[0].source, "b-auth");
    }

    #[test]
    fn reserved_fields_are_refused() {
        let p = policy(
            "sneaky",
            0,
            json!({ "targetRefs": [{ "name": "internal-gateway" }], "mergeType": "StrategicMerge" }),
        );

        // Nothing survives the filter, so there is nothing to enforce and no
        // policy is written - the redirect to another gateway must not happen.
        let compiled = Compositor::new("public-gateway").compile(&[p], &[]);

        assert!(compiled.spec.is_none());
        assert_eq!(compiled.conflicts.len(), 2);
    }

    #[test]
    fn reserved_targetrefs_cannot_redirect_a_real_policy() {
        let p = policy(
            "sneaky",
            0,
            json!({
                "targetRefs": [{ "name": "internal-gateway" }],
                "authorization": { "rules": [{ "name": "keep" }] },
            }),
        );

        let compiled = Compositor::new("public-gateway").compile(&[p], &[]);
        let spec = compiled.spec.unwrap();

        assert_eq!(spec["targetRefs"][0]["name"], "public-gateway");
        assert_eq!(spec["targetRefs"].as_array().unwrap().len(), 1);
        assert_eq!(compiled.conflicts.len(), 1);
    }

    #[test]
    fn conflicting_default_actions_keep_the_first_and_report() {
        let a = policy(
            "a",
            0,
            json!({ "authorization": { "defaultAction": "Allow" } }),
        );
        let b = policy(
            "b",
            1,
            json!({ "authorization": { "defaultAction": "Deny" } }),
        );

        let compiled = Compositor::new("public-gateway").compile(&[a, b], &[]);
        let spec = compiled.spec.unwrap();

        assert_eq!(spec["authorization"]["defaultAction"], "Allow");
        assert_eq!(compiled.conflicts.len(), 1);
    }

    #[test]
    fn a_template_may_not_impersonate_the_generated_rule() {
        let p = policy(
            "impostor",
            0,
            json!({ "authorization": { "rules": [{ "name": BLOCK_RULE_NAME, "action": "Allow" }] } }),
        );

        let compiled = Compositor::new("public-gateway").compile(&[p], &[]);
        assert_eq!(compiled.conflicts.len(), 1);
        assert!(
            compiled.spec.is_none()
                || compiled.spec.unwrap()["authorization"]["rules"]
                    .as_array()
                    .unwrap()
                    .is_empty()
        );
    }

    #[test]
    fn active_cidrs_skips_other_gateways_and_expired_blocks() {
        let now = chrono::Utc::now();

        let mut other = block("other", "198.51.100.1/32");
        other.spec.gateway = "internal-gateway".to_string();

        let mut expired = block("expired", "198.51.100.2/32");
        expired.spec.expires_at = Some("2000-01-01T00:00:00Z".to_string());

        let keep = block("keep", "203.0.113.4");

        let cidrs = Compositor::new("public-gateway").active_cidrs(&[other, expired, keep], now);

        assert_eq!(cidrs.len(), 1);
        assert_eq!(cidrs[0].to_string(), "203.0.113.4/32");
    }

    #[test]
    fn duplicate_cidrs_collapse() {
        let compiled = Compositor::new("public-gateway")
            .compile(&[], &[net("203.0.113.4/32"), net("203.0.113.4/32")]);
        let spec = compiled.spec.unwrap();

        let cidrs = spec["authorization"]["rules"][0]["principal"]["clientCIDRs"]
            .as_array()
            .unwrap();
        assert_eq!(cidrs.len(), 1);
    }
}
