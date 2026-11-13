use std::collections::BTreeSet;

use registry_api::{HarborAccess, HarborAction};
use serde::Deserialize;
use wildmatch::WildMatch;

use super::{ForgejoClaims, Principal, oidc::groups_match};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    #[serde(default)]
    pub kanidm: Option<KanidmSubject>,
    #[serde(default)]
    pub forgejo: Option<ForgejoSubject>,
    #[serde(default)]
    pub token: Option<TokenSubject>,
    pub allow: Vec<Allow>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KanidmSubject {
    pub groups: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgejoSubject {
    pub repository: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub pull_requests: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenSubject {}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allow {
    pub project: String,
    pub actions: Vec<HarborAction>,
}

impl Grant {
    pub fn validate(&self) -> Result<(), String> {
        let subjects = [
            self.kanidm.is_some(),
            self.forgejo.is_some(),
            self.token.is_some(),
        ];
        if subjects.iter().filter(|s| **s).count() != 1 {
            return Err("set exactly one of kanidm, forgejo or token".to_owned());
        }
        if self.kanidm.as_ref().is_some_and(|k| k.groups.is_empty()) {
            return Err("kanidm.groups must not be empty".to_owned());
        }
        if self.allow.iter().any(|a| a.actions.is_empty()) {
            return Err("every allow entry needs at least one action".to_owned());
        }
        Ok(())
    }

    fn matches(&self, principal: &Principal) -> bool {
        match principal {
            Principal::User { groups, .. } => self
                .kanidm
                .as_ref()
                .is_some_and(|k| groups_match(groups, &k.groups)),
            Principal::Forgejo(claims) => self.forgejo.as_ref().is_some_and(|f| f.matches(claims)),
            Principal::Token(_) => self.token.is_some(),
        }
    }
}

impl ForgejoSubject {
    fn matches(&self, claims: &ForgejoClaims) -> bool {
        let pull_request = claims
            .event_name
            .as_deref()
            .is_some_and(|e| e.starts_with("pull_request"));
        if pull_request && !self.pull_requests {
            return false;
        }

        if !WildMatch::new(&self.repository.to_lowercase())
            .matches(&claims.repository.to_lowercase())
        {
            return false;
        }

        if let Some(pattern) = &self.git_ref
            && !claims
                .git_ref
                .as_deref()
                .is_some_and(|r| WildMatch::new(pattern).matches(r))
        {
            return false;
        }

        if let Some(pattern) = &self.workflow
            && !claims
                .workflow_ref
                .as_deref()
                .and_then(workflow_file)
                .is_some_and(|w| WildMatch::new(pattern).matches(w))
        {
            return false;
        }

        true
    }
}

fn workflow_file(workflow_ref: &str) -> Option<&str> {
    let path = workflow_ref
        .split_once('@')
        .map_or(workflow_ref, |(p, _)| p);
    path.rsplit('/').next().filter(|f| !f.is_empty())
}

pub fn allowed(grants: &[Grant], principal: &Principal, project: &str) -> BTreeSet<HarborAction> {
    grants
        .iter()
        .filter(|g| g.matches(principal))
        .flat_map(|g| &g.allow)
        .filter(|a| WildMatch::new(&a.project).matches(project))
        .flat_map(|a| a.actions.iter().copied())
        .collect()
}

pub fn validate_request(access: &[HarborAccess]) -> Result<(), String> {
    if access.is_empty() {
        return Err("request at least one project".to_owned());
    }
    if access.len() > 32 {
        return Err("at most 32 projects per token".to_owned());
    }
    for item in access {
        let valid = !item.project.is_empty()
            && item.project.len() <= 255
            && item
                .project
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
        if !valid {
            return Err(format!("{:?} is not a harbor project name", item.project));
        }
        if item.actions.is_empty() {
            return Err(format!("{}: request at least one action", item.project));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants(yaml: &str) -> Vec<Grant> {
        let grants: Vec<Grant> = serde_yaml::from_str(yaml).unwrap();
        for grant in &grants {
            grant.validate().unwrap();
        }
        grants
    }

    fn forgejo(repository: &str, git_ref: &str, event: &str) -> Principal {
        Principal::Forgejo(ForgejoClaims {
            repository: repository.to_owned(),
            git_ref: Some(git_ref.to_owned()),
            event_name: Some(event.to_owned()),
            workflow_ref: Some(format!(
                "{repository}/.forgejo/workflows/build-app.yaml@{git_ref}"
            )),
            ..ForgejoClaims::default()
        })
    }

    fn user(groups: &[&str]) -> Principal {
        Principal::User {
            name: "someone".to_owned(),
            groups: groups.iter().map(|g| g.to_string()).collect(),
            admin: false,
            reader: true,
        }
    }

    const GRANTS: &str = r#"
- kanidm: {groups: [registry_users]}
  allow: [{project: ortzi-*, actions: [pull]}]
- forgejo: {repository: "*"}
  allow: [{project: containers, actions: [pull]}]
- forgejo: {repository: Anurag/K8s, ref: refs/heads/main, workflow: build-*.yaml}
  allow: [{project: containers, actions: [push]}]
"#;

    #[test]
    fn users_match_on_groups() {
        let grants = grants(GRANTS);
        let reader = user(&["registry_users@idm.example.test"]);
        assert_eq!(
            allowed(&grants, &reader, "ortzi-packages"),
            BTreeSet::from([HarborAction::Pull])
        );
        assert!(allowed(&grants, &reader, "containers").is_empty());
        assert!(allowed(&grants, &user(&["other"]), "ortzi-packages").is_empty());
    }

    #[test]
    fn forgejo_grants_union_across_rules() {
        let grants = grants(GRANTS);
        assert_eq!(
            allowed(
                &grants,
                &forgejo("anurag/k8s", "refs/heads/main", "push"),
                "containers"
            ),
            BTreeSet::from([HarborAction::Pull, HarborAction::Push])
        );
        assert_eq!(
            allowed(
                &grants,
                &forgejo("anurag/k8s", "refs/heads/feature", "push"),
                "containers"
            ),
            BTreeSet::from([HarborAction::Pull])
        );
        assert_eq!(
            allowed(
                &grants,
                &forgejo("someone/else", "refs/heads/main", "push"),
                "containers"
            ),
            BTreeSet::from([HarborAction::Pull])
        );
    }

    #[test]
    fn pull_requests_are_excluded_unless_opted_in() {
        let principal = forgejo("anurag/k8s", "refs/heads/main", "pull_request");
        assert!(allowed(&grants(GRANTS), &principal, "containers").is_empty());

        let opted_in = grants(
            r#"
- forgejo: {repository: "*", pull_requests: true}
  allow: [{project: containers, actions: [pull]}]
"#,
        );
        assert_eq!(
            allowed(&opted_in, &principal, "containers"),
            BTreeSet::from([HarborAction::Pull])
        );
    }

    #[test]
    fn grants_need_exactly_one_subject() {
        let grants: Vec<Grant> = serde_yaml::from_str(
            r#"
- allow: [{project: x, actions: [pull]}]
- token: {}
  kanidm: {groups: [a]}
  allow: [{project: x, actions: [pull]}]
"#,
        )
        .unwrap();
        assert!(grants.iter().all(|g| g.validate().is_err()));
    }

    #[test]
    fn requests_are_validated() {
        let access = |project: &str, actions: Vec<HarborAction>| HarborAccess {
            project: project.to_owned(),
            actions,
            package: None,
        };
        assert!(validate_request(&[]).is_err());
        assert!(validate_request(&[access("*", vec![HarborAction::Pull])]).is_err());
        assert!(validate_request(&[access("containers", vec![])]).is_err());
        assert!(validate_request(&[access("ortzi-packages", vec![HarborAction::Pull])]).is_ok());
    }

    #[test]
    fn workflow_file_is_the_last_path_segment() {
        assert_eq!(
            workflow_file("anurag/k8s/.forgejo/workflows/build.yaml@refs/heads/main"),
            Some("build.yaml")
        );
    }
}
