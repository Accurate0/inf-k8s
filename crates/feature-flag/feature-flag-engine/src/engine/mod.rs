//! The pure, side-effect-free evaluation engine. An [`Engine`] wraps an immutable
//! [`Snapshot`] and resolves flags against an [`EvalContext`], yielding a value, the
//! served variant, and a reason. Kept free of IO so it can be exhaustively tested.

mod types;

pub use types::{ErrorCode, EvalContext, EvalError, Reason, Resolution};

use crate::model::{Constraint, ConstraintGroup, Flag, Operator, Rule, Segment, Snapshot};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone)]
pub struct Engine {
    snapshot: Arc<Snapshot>,
}

impl Engine {
    pub fn new(snapshot: Arc<Snapshot>) -> Self {
        Self { snapshot }
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Resolve a single flag. The returned [`Resolution`] always carries a concrete
    /// value drawn from the flag's variants; type checking against the caller's
    /// requested type happens in [`crate::convert`].
    pub fn evaluate(&self, flag_key: &str, ctx: &EvalContext) -> Result<Resolution, EvalError> {
        self.evaluate_inner(flag_key, ctx, &mut Vec::new())
    }

    fn evaluate_inner(
        &self,
        flag_key: &str,
        ctx: &EvalContext,
        stack: &mut Vec<String>,
    ) -> Result<Resolution, EvalError> {
        if stack.iter().any(|k| k == flag_key) {
            return Err(EvalError {
                code: ErrorCode::ParseError,
                message: format!("prerequisite cycle through flag `{flag_key}`"),
            });
        }

        let flag = self.snapshot.flags.get(flag_key).ok_or_else(|| EvalError {
            code: ErrorCode::FlagNotFound,
            message: format!("flag `{flag_key}` not found"),
        })?;

        if flag.archived || !flag.enabled {
            return Self::resolve_variant(flag, &flag.default_variant_key, Reason::Disabled);
        }

        for prereq in &flag.prerequisites {
            stack.push(flag_key.to_string());
            let result = self.evaluate_inner(&prereq.flag_key, ctx, stack);
            stack.pop();

            let met = matches!(&result, Ok(res) if res.variant == prereq.variant_key);
            if !met {
                return Self::resolve_variant(flag, &flag.default_variant_key, Reason::Default);
            }
        }

        for rule in &flag.rules {
            if !self.rule_matches(rule, ctx) {
                continue;
            }
            if !rule.distributions.is_empty() {
                let variant_key = Self::pick_distribution(flag_key, rule, ctx);
                return Self::resolve_variant(flag, variant_key, Reason::Split);
            }
            if let Some(variant_key) = &rule.variant_key {
                return Self::resolve_variant(flag, variant_key, Reason::TargetingMatch);
            }
        }

        Self::resolve_variant(flag, &flag.default_variant_key, Reason::Default)
    }

    /// A rule matches when its referenced segment (if any) matches AND all of its
    /// inline constraints match.
    fn rule_matches(&self, rule: &Rule, ctx: &EvalContext) -> bool {
        let segment_ok = match rule.segment_key.as_deref() {
            None => true,
            Some(key) => match self.snapshot.segments.get(key) {
                Some(segment) => Self::segment_matches(segment, ctx),
                // A rule pointing at a missing segment never matches rather than erroring.
                None => false,
            },
        };
        segment_ok
            && rule
                .constraint_groups
                .iter()
                .all(|g| Self::group_matches(g, ctx))
    }

    /// A constraint group matches when any of its constraints match (OR). An empty
    /// group matches, so it never blocks a rule.
    fn group_matches(group: &ConstraintGroup, ctx: &EvalContext) -> bool {
        group.constraints.is_empty()
            || group
                .constraints
                .iter()
                .any(|c| Self::constraint_matches(c, ctx))
    }

    fn resolve_variant(
        flag: &Flag,
        variant_key: &str,
        reason: Reason,
    ) -> Result<Resolution, EvalError> {
        let variant = flag.variant(variant_key).ok_or_else(|| EvalError {
            code: ErrorCode::ParseError,
            message: format!(
                "flag `{}` references unknown variant `{variant_key}`",
                flag.key
            ),
        })?;
        Ok(Resolution {
            value: variant.value.clone(),
            variant: variant.key.clone(),
            reason,
        })
    }

    pub fn segment_matches(segment: &Segment, ctx: &EvalContext) -> bool {
        Self::constraints_match(&segment.constraints, ctx)
    }

    /// A context matches a constraint set only when every constraint matches (AND).
    fn constraints_match(constraints: &[Constraint], ctx: &EvalContext) -> bool {
        constraints.iter().all(|c| Self::constraint_matches(c, ctx))
    }

    fn constraint_matches(c: &Constraint, ctx: &EvalContext) -> bool {
        let attr = ctx.attributes.get(&c.attribute);
        let first = c.values.first();

        match c.operator {
            Operator::Exists => attr.is_some(),
            Operator::Eq => attr.is_some_and(|a| first == Some(a)),
            Operator::Neq => attr.is_none_or(|a| first != Some(a)),
            Operator::In => attr.is_some_and(|a| c.values.iter().any(|v| v == a)),
            Operator::NotIn => attr.is_none_or(|a| !c.values.iter().any(|v| v == a)),
            Operator::Contains => Self::string_op(attr, first, |a, b| a.contains(b)),
            Operator::StartsWith => Self::string_op(attr, first, |a, b| a.starts_with(b)),
            Operator::EndsWith => Self::string_op(attr, first, |a, b| a.ends_with(b)),
            Operator::Regex => Self::string_op(attr, first, Self::regex_match),
            Operator::Gt => Self::number_op(attr, first, |a, b| a > b),
            Operator::Gte => Self::number_op(attr, first, |a, b| a >= b),
            Operator::Lt => Self::number_op(attr, first, |a, b| a < b),
            Operator::Lte => Self::number_op(attr, first, |a, b| a <= b),
        }
    }

    fn string_op(
        attr: Option<&Value>,
        operand: Option<&Value>,
        f: impl Fn(&str, &str) -> bool,
    ) -> bool {
        match (attr.and_then(Value::as_str), operand.and_then(Value::as_str)) {
            (Some(a), Some(b)) => f(a, b),
            _ => false,
        }
    }

    fn number_op(
        attr: Option<&Value>,
        operand: Option<&Value>,
        f: impl Fn(f64, f64) -> bool,
    ) -> bool {
        match (attr.and_then(Value::as_f64), operand.and_then(Value::as_f64)) {
            (Some(a), Some(b)) => f(a, b),
            _ => false,
        }
    }

    /// Size-bounded so a pathological pattern can't exhaust memory; an uncompilable
    /// or oversized pattern never matches rather than failing the resolution.
    fn regex_match(haystack: &str, pattern: &str) -> bool {
        const SIZE_LIMIT: usize = 1 << 20;
        regex::RegexBuilder::new(pattern)
            .size_limit(SIZE_LIMIT)
            .dfa_size_limit(SIZE_LIMIT)
            .build()
            .is_ok_and(|re| re.is_match(haystack))
    }

    /// Deterministic bucketing into [0,100), walking the cumulative weights. The rule's
    /// salt lets two rollouts on one flag bucket the same key independently.
    fn pick_distribution<'a>(flag_key: &str, rule: &'a Rule, ctx: &EvalContext) -> &'a str {
        let bucket = Self::bucket_of(flag_key, &rule.bucket_salt, &ctx.targeting_key);

        let mut cumulative = 0u32;
        for d in &rule.distributions {
            cumulative += d.weight;
            if bucket < cumulative {
                return &d.variant_key;
            }
        }

        &rule.distributions[rule.distributions.len() - 1].variant_key
    }

    /// An empty salt reproduces the pre-salt `flag_key:targeting_key` hash, so existing
    /// rollouts keep their buckets.
    fn bucket_of(flag_key: &str, salt: &str, targeting_key: &str) -> u32 {
        let mut hasher = Sha256::new();
        hasher.update(flag_key.as_bytes());
        if !salt.is_empty() {
            hasher.update(b":");
            hasher.update(salt.as_bytes());
        }
        hasher.update(b":");
        hasher.update(targeting_key.as_bytes());

        let digest = hasher.finalize();
        let n = u64::from_be_bytes(digest[..8].try_into().unwrap());
        (n % 100) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use serde_json::json;

    fn bool_flag() -> Flag {
        Flag {
            key: "feature".into(),
            value_type: ValueType::Boolean,
            enabled: true,
            default_variant_key: "off".into(),
            archived: false,
            variants: vec![
                Variant { key: "on".into(), value: json!(true) },
                Variant { key: "off".into(), value: json!(false) },
            ],
            rules: vec![],
            prerequisites: vec![],
        }
    }

    fn engine(flag: Flag, segments: Vec<Segment>) -> Engine {
        let mut s = Snapshot { version: 1, ..Default::default() };
        s.flags.insert(flag.key.clone(), flag);
        for seg in segments {
            s.segments.insert(seg.key.clone(), seg);
        }
        Engine::new(Arc::new(s))
    }

    fn ctx(targeting_key: &str, attrs: serde_json::Value) -> EvalContext {
        EvalContext {
            targeting_key: targeting_key.into(),
            attributes: attrs
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    fn engine_with(flags: Vec<Flag>) -> Engine {
        let mut s = Snapshot { version: 1, ..Default::default() };
        for f in flags {
            s.flags.insert(f.key.clone(), f);
        }
        Engine::new(Arc::new(s))
    }

    #[test]
    fn prerequisite_met_allows_rules() {
        let mut master = bool_flag();
        master.key = "master".into();
        master.default_variant_key = "on".into();

        let mut dependent = bool_flag();
        dependent.key = "dependent".into();
        dependent.default_variant_key = "on".into();
        dependent.prerequisites = vec![Prerequisite {
            flag_key: "master".into(),
            variant_key: "on".into(),
        }];

        let e = engine_with(vec![master, dependent]);
        let r = e.evaluate("dependent", &ctx("u1", json!({}))).unwrap();
        assert_eq!(r.variant, "on");
    }

    #[test]
    fn prerequisite_unmet_serves_default() {
        let mut master = bool_flag();
        master.key = "master".into();
        master.default_variant_key = "off".into(); // resolves to "off"

        let mut dependent = bool_flag();
        dependent.key = "dependent".into();
        dependent.default_variant_key = "off".into();
        dependent.rules = vec![Rule {
            rank: 0,
            segment_key: None,
            variant_key: Some("on".into()),
            distributions: vec![],
            constraint_groups: vec![],
            bucket_salt: String::new(),
        }];
        dependent.prerequisites = vec![Prerequisite {
            flag_key: "master".into(),
            variant_key: "on".into(), // master is "off", so unmet
        }];

        let e = engine_with(vec![master, dependent]);
        let r = e.evaluate("dependent", &ctx("u1", json!({}))).unwrap();
        // Rule would have served "on", but the unmet prerequisite forces the default.
        assert_eq!(r.variant, "off");
        assert_eq!(r.reason, Reason::Default);
    }

    #[test]
    fn prerequisite_cycle_resolves_to_default() {
        let mut a = bool_flag();
        a.key = "a".into();
        a.default_variant_key = "off".into();
        a.prerequisites = vec![Prerequisite { flag_key: "b".into(), variant_key: "on".into() }];

        let mut b = bool_flag();
        b.key = "b".into();
        b.default_variant_key = "off".into();
        b.prerequisites = vec![Prerequisite { flag_key: "a".into(), variant_key: "on".into() }];

        let e = engine_with(vec![a, b]);
        // The cycle is treated as an unmet prerequisite, not an infinite loop or error.
        let r = e.evaluate("a", &ctx("u1", json!({}))).unwrap();
        assert_eq!(r.variant, "off");
    }

    #[test]
    fn default_when_no_rules() {
        let e = engine(bool_flag(), vec![]);
        let r = e.evaluate("feature", &ctx("u1", json!({}))).unwrap();
        assert_eq!(r.value, json!(false));
        assert_eq!(r.variant, "off");
        assert_eq!(r.reason, Reason::Default);
    }

    #[test]
    fn disabled_serves_default_with_disabled_reason() {
        let mut flag = bool_flag();
        flag.enabled = false;
        let e = engine(flag, vec![]);
        let r = e.evaluate("feature", &ctx("u1", json!({}))).unwrap();
        assert_eq!(r.reason, Reason::Disabled);
    }

    #[test]
    fn missing_flag_errors() {
        let e = engine(bool_flag(), vec![]);
        let err = e.evaluate("nope", &ctx("u1", json!({}))).unwrap_err();
        assert_eq!(err.code, ErrorCode::FlagNotFound);
    }

    #[test]
    fn segment_targeting_match() {
        let mut flag = bool_flag();
        flag.rules = vec![Rule {
            rank: 0,
            bucket_salt: String::new(),
            segment_key: Some("beta".into()),
            variant_key: Some("on".into()),
            distributions: vec![],
            constraint_groups: vec![],
        }];
        let beta = Segment {
            key: "beta".into(),
            name: "Beta".into(),
            constraints: vec![Constraint {
                attribute: "email".into(),
                operator: Operator::EndsWith,
                values: vec![json!("@anurag.sh")],
            }],
        };
        let e = engine(flag, vec![beta]);

        let hit = e.evaluate("feature", &ctx("u1", json!({"email": "a@anurag.sh"}))).unwrap();
        assert_eq!(hit.reason, Reason::TargetingMatch);
        assert_eq!(hit.value, json!(true));

        let miss = e.evaluate("feature", &ctx("u2", json!({"email": "a@other.com"}))).unwrap();
        assert_eq!(miss.reason, Reason::Default);
    }

    fn group(constraints: Vec<Constraint>) -> ConstraintGroup {
        ConstraintGroup { constraints }
    }

    #[test]
    fn inline_constraints_match_without_segment() {
        let mut flag = bool_flag();
        flag.rules = vec![Rule {
            rank: 0,
            bucket_salt: String::new(),
            segment_key: None,
            variant_key: Some("on".into()),
            distributions: vec![],
            constraint_groups: vec![group(vec![Constraint {
                attribute: "email".into(),
                operator: Operator::EndsWith,
                values: vec![json!("@anurag.sh")],
            }])],
        }];
        let e = engine(flag, vec![]);

        let hit = e.evaluate("feature", &ctx("u1", json!({"email": "a@anurag.sh"}))).unwrap();
        assert_eq!(hit.reason, Reason::TargetingMatch);
        assert_eq!(hit.value, json!(true));

        let miss = e.evaluate("feature", &ctx("u2", json!({"email": "a@other.com"}))).unwrap();
        assert_eq!(miss.reason, Reason::Default);
    }

    #[test]
    fn inline_constraint_group_is_ored() {
        let mut flag = bool_flag();
        // One group with two constraints: match when country is AU OR NZ.
        flag.rules = vec![Rule {
            rank: 0,
            bucket_salt: String::new(),
            segment_key: None,
            variant_key: Some("on".into()),
            distributions: vec![],
            constraint_groups: vec![group(vec![
                Constraint { attribute: "country".into(), operator: Operator::Eq, values: vec![json!("AU")] },
                Constraint { attribute: "country".into(), operator: Operator::Eq, values: vec![json!("NZ")] },
            ])],
        }];
        let e = engine(flag, vec![]);

        assert_eq!(e.evaluate("feature", &ctx("u1", json!({"country": "AU"}))).unwrap().reason, Reason::TargetingMatch);
        assert_eq!(e.evaluate("feature", &ctx("u2", json!({"country": "NZ"}))).unwrap().reason, Reason::TargetingMatch);
        assert_eq!(e.evaluate("feature", &ctx("u3", json!({"country": "US"}))).unwrap().reason, Reason::Default);
    }

    #[test]
    fn inline_constraint_groups_are_anded() {
        let mut flag = bool_flag();
        // (country == AU OR NZ) AND (plan == pro).
        flag.rules = vec![Rule {
            rank: 0,
            bucket_salt: String::new(),
            segment_key: None,
            variant_key: Some("on".into()),
            distributions: vec![],
            constraint_groups: vec![
                group(vec![
                    Constraint { attribute: "country".into(), operator: Operator::Eq, values: vec![json!("AU")] },
                    Constraint { attribute: "country".into(), operator: Operator::Eq, values: vec![json!("NZ")] },
                ]),
                group(vec![Constraint { attribute: "plan".into(), operator: Operator::Eq, values: vec![json!("pro")] }]),
            ],
        }];
        let e = engine(flag, vec![]);

        assert_eq!(e.evaluate("feature", &ctx("u1", json!({"country": "AU", "plan": "pro"}))).unwrap().reason, Reason::TargetingMatch);
        assert_eq!(e.evaluate("feature", &ctx("u2", json!({"country": "NZ", "plan": "free"}))).unwrap().reason, Reason::Default);
        assert_eq!(e.evaluate("feature", &ctx("u3", json!({"country": "US", "plan": "pro"}))).unwrap().reason, Reason::Default);
    }

    #[test]
    fn segment_and_inline_constraints_are_anded() {
        let mut flag = bool_flag();
        flag.rules = vec![Rule {
            rank: 0,
            bucket_salt: String::new(),
            segment_key: Some("beta".into()),
            variant_key: Some("on".into()),
            distributions: vec![],
            constraint_groups: vec![group(vec![Constraint {
                attribute: "age".into(),
                operator: Operator::Gte,
                values: vec![json!(18)],
            }])],
        }];
        let beta = Segment {
            key: "beta".into(),
            name: "Beta".into(),
            constraints: vec![Constraint {
                attribute: "plan".into(),
                operator: Operator::Eq,
                values: vec![json!("pro")],
            }],
        };
        let e = engine(flag, vec![beta]);

        let both = e.evaluate("feature", &ctx("u1", json!({"plan": "pro", "age": 20}))).unwrap();
        assert_eq!(both.reason, Reason::TargetingMatch);

        let segment_only = e.evaluate("feature", &ctx("u2", json!({"plan": "pro", "age": 16}))).unwrap();
        assert_eq!(segment_only.reason, Reason::Default);

        let constraint_only = e.evaluate("feature", &ctx("u3", json!({"plan": "free", "age": 20}))).unwrap();
        assert_eq!(constraint_only.reason, Reason::Default);
    }

    #[test]
    fn rules_evaluated_in_order() {
        let mut flag = bool_flag();
        flag.rules = vec![
            Rule { rank: 0, segment_key: None, variant_key: Some("on".into()), distributions: vec![], constraint_groups: vec![], bucket_salt: String::new() },
            Rule { rank: 1, segment_key: None, variant_key: Some("off".into()), distributions: vec![], constraint_groups: vec![], bucket_salt: String::new() },
        ];
        let e = engine(flag, vec![]);
        let r = e.evaluate("feature", &ctx("u1", json!({}))).unwrap();
        assert_eq!(r.variant, "on");
    }

    #[test]
    fn percentage_rollout_is_deterministic_and_split() {
        let mut flag = bool_flag();
        flag.rules = vec![Rule {
            rank: 0,
            bucket_salt: String::new(),
            segment_key: None,
            variant_key: None,
            distributions: vec![
                Distribution { variant_key: "on".into(), weight: 50 },
                Distribution { variant_key: "off".into(), weight: 50 },
            ],
            constraint_groups: vec![],
        }];
        let e = engine(flag, vec![]);

        let a1 = e.evaluate("feature", &ctx("stable-key", json!({}))).unwrap();
        let a2 = e.evaluate("feature", &ctx("stable-key", json!({}))).unwrap();
        assert_eq!(a1.variant, a2.variant);
        assert_eq!(a1.reason, Reason::Split);

        let on = (0..1000)
            .filter(|i| {
                e.evaluate("feature", &ctx(&format!("user-{i}"), json!({})))
                    .unwrap()
                    .variant
                    == "on"
            })
            .count();
        assert!((350..650).contains(&on), "unexpected split: {on}/1000");
    }

    #[test]
    fn bucket_salt_makes_rollouts_independent() {
        // Two 50/50 rollouts on the same flag with different salts should not bucket
        // every key the same way; without a salt they would be perfectly correlated.
        let agree = (0..1000)
            .filter(|i| {
                let key = format!("user-{i}");
                let a = Engine::bucket_of("feature", "salt-a", &key) < 50;
                let b = Engine::bucket_of("feature", "salt-b", &key) < 50;
                a == b
            })
            .count();
        assert!((400..600).contains(&agree), "salts not independent: {agree}/1000 agree");

        // The same salt is deterministic.
        assert_eq!(
            Engine::bucket_of("feature", "salt-a", "user-1"),
            Engine::bucket_of("feature", "salt-a", "user-1"),
        );
    }

    #[test]
    fn empty_salt_preserves_legacy_bucketing() {
        // An empty salt must hash `flag_key:targeting_key` exactly as before salts
        // existed, so deploying this change does not re-bucket existing rollouts.
        for key in ["user-1", "abc", "", "🦀"] {
            let mut hasher = Sha256::new();
            hasher.update(b"feature");
            hasher.update(b":");
            hasher.update(key.as_bytes());
            let legacy = (u64::from_be_bytes(hasher.finalize()[..8].try_into().unwrap()) % 100) as u32;
            assert_eq!(Engine::bucket_of("feature", "", key), legacy);
        }
    }

    #[test]
    fn regex_operator_matches_and_rejects() {
        assert!(Engine::regex_match("v12", "^v[0-9]+$"));
        assert!(!Engine::regex_match("v12a", "^v[0-9]+$"));
        assert!(Engine::regex_match("hello world", "wor"));
        // An invalid pattern never matches rather than erroring.
        assert!(!Engine::regex_match("anything", "("));
    }

    #[test]
    fn numeric_and_membership_operators() {
        let seg = Segment {
            key: "s".into(),
            name: "s".into(),
            constraints: vec![
                Constraint { attribute: "age".into(), operator: Operator::Gte, values: vec![json!(18)] },
                Constraint { attribute: "country".into(), operator: Operator::In, values: vec![json!("AU"), json!("NZ")] },
            ],
        };
        assert!(Engine::segment_matches(&seg, &ctx("u", json!({"age": 20, "country": "AU"}))));
        assert!(!Engine::segment_matches(&seg, &ctx("u", json!({"age": 16, "country": "AU"}))));
        assert!(!Engine::segment_matches(&seg, &ctx("u", json!({"age": 20, "country": "US"}))));
    }
}
