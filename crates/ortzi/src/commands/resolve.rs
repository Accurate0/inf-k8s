use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
};

use anyhow::bail;
use registry_api::{Target, VersionSummary, is_release, requirement, satisfies, version_key};

use crate::store::Installed;

pub(crate) trait Catalog {
    type Release;

    async fn versions(&self, name: &str) -> anyhow::Result<Vec<VersionSummary>>;

    async fn release(
        &self,
        name: &str,
        version: &str,
    ) -> anyhow::Result<(Self::Release, BTreeMap<String, String>)>;
}

pub struct Root<'a> {
    pub name: &'a str,
    pub version: Option<&'a str>,
    pub requested: bool,
}

pub struct Step<R> {
    pub name: String,
    pub version: String,
    pub release: R,
    pub dependencies: BTreeMap<String, String>,
    pub requested: bool,
    pub root: bool,
}

pub struct Plan<R> {
    pub steps: Vec<Step<R>>,
    pub held: Vec<String>,
}

struct Resolver<'a, C: Catalog> {
    catalog: &'a C,
    installed: &'a Installed,
    target: Target,
    force: bool,
    ranges: BTreeMap<String, BTreeMap<String, String>>,
    chosen: BTreeMap<String, Step<C::Release>>,
    exact: BTreeSet<String>,
    order: Vec<String>,
    stack: Vec<String>,
    held: Vec<String>,
}

pub(crate) async fn resolve<C: Catalog>(
    catalog: &C,
    installed: &Installed,
    target: Target,
    roots: &[Root<'_>],
    force: bool,
) -> anyhow::Result<Plan<C::Release>> {
    let mut ranges: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (from, package) in &installed.packages {
        for (dependency, req) in &package.dependencies {
            ranges
                .entry(dependency.clone())
                .or_default()
                .insert(from.clone(), req.clone());
        }
    }

    let mut resolver = Resolver {
        catalog,
        installed,
        target,
        force,
        ranges,
        chosen: BTreeMap::new(),
        exact: BTreeSet::new(),
        order: Vec::new(),
        stack: Vec::new(),
        held: Vec::new(),
    };
    for root in roots {
        resolver
            .choose(root.name.to_owned(), root.version, root.requested, true)
            .await?;
    }

    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    for name in resolver.order.clone() {
        resolver.visit(&name, &mut seen, &mut ordered);
    }
    let steps = ordered
        .into_iter()
        .filter_map(|name| resolver.chosen.remove(&name))
        .collect();
    Ok(Plan {
        steps,
        held: resolver.held,
    })
}

impl<C: Catalog> Resolver<'_, C> {
    fn visit(&self, name: &str, seen: &mut BTreeSet<String>, ordered: &mut Vec<String>) {
        let Some(step) = self.chosen.get(name) else {
            return;
        };
        if !seen.insert(name.to_owned()) {
            return;
        }
        for dependency in step.dependencies.keys() {
            self.visit(dependency, seen, ordered);
        }
        ordered.push(name.to_owned());
    }

    fn constraints(&self, name: &str) -> Vec<(String, String)> {
        self.ranges
            .get(name)
            .map(|from| from.iter().map(|(f, r)| (f.clone(), r.clone())).collect())
            .unwrap_or_default()
    }

    fn fits(&self, name: &str, version: &str) -> anyhow::Result<bool> {
        for (_, req) in self.constraints(name) {
            if !satisfies(&requirement(&req).map_err(anyhow::Error::msg)?, version) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn describe(&self, name: &str) -> String {
        self.constraints(name)
            .iter()
            .map(|(from, req)| format!("{from} needs {req}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn chain(&self, name: &str) -> String {
        self.stack
            .iter()
            .map(String::as_str)
            .chain([name])
            .collect::<Vec<_>>()
            .join(" → ")
    }

    fn choose<'s>(
        &'s mut self,
        name: String,
        exact: Option<&'s str>,
        requested: bool,
        root: bool,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 's>> {
        Box::pin(async move {
            if self.stack.contains(&name) {
                bail!("dependency cycle: {}", self.chain(&name));
            }

            if let Some(step) = self.chosen.get_mut(&name) {
                step.requested |= requested;
                step.root |= root;
                let version = step.version.clone();
                if exact.is_none_or(|v| v == version) && self.fits(&name, &version)? {
                    return Ok(());
                }
                if self.exact.contains(&name) {
                    bail!(
                        "{name}@{version} was asked for, but {}",
                        self.describe(&name)
                    );
                }
            }

            let installed = self.installed.packages.get(&name);
            let version = match exact {
                Some(version) => {
                    if !self.force && !self.fits(&name, version)? {
                        bail!(
                            "{name}@{version} doesn't fit what's installed: {}; pass --force to install it anyway",
                            self.describe(&name)
                        );
                    }
                    self.exact.insert(name.clone());
                    version.to_owned()
                }
                None => {
                    if !root
                        && let Some(installed) = installed
                        && self.fits(&name, &installed.version)?
                    {
                        return Ok(());
                    }
                    if !root && let Some(installed) = installed.filter(|p| p.pinned) {
                        bail!(
                            "{name} is pinned at {} but {}; run `ortzi unpin {name}`",
                            installed.version,
                            self.describe(&name)
                        );
                    }
                    self.pick(&name, root).await?
                }
            };

            let (release, dependencies) = self.catalog.release(&name, &version).await?;
            for from in self.ranges.values_mut() {
                from.remove(&name);
            }
            for (dependency, req) in &dependencies {
                self.ranges
                    .entry(dependency.clone())
                    .or_default()
                    .insert(name.clone(), req.clone());
            }

            let requested = requested || self.chosen.get(&name).is_some_and(|s| s.requested);
            let root = root || self.chosen.get(&name).is_some_and(|s| s.root);
            if !self.order.contains(&name) {
                self.order.push(name.clone());
            }
            self.chosen.insert(
                name.clone(),
                Step {
                    name: name.clone(),
                    version,
                    release,
                    dependencies: dependencies.clone(),
                    requested,
                    root,
                },
            );

            self.stack.push(name);
            for dependency in dependencies.into_keys() {
                self.choose(dependency, None, false, false).await?;
            }
            self.stack.pop();
            Ok(())
        })
    }

    async fn pick(&mut self, name: &str, root: bool) -> anyhow::Result<String> {
        let target = self.target;
        let versions: Vec<VersionSummary> = self
            .catalog
            .versions(name)
            .await?
            .into_iter()
            .filter(|v| !v.yanked)
            .filter(|v| v.targets.contains(&target) || v.targets.contains(&Target::Wasm))
            .filter(|v| semver::Version::parse(&v.version).is_ok_and(|s| is_release(&s)))
            .collect();
        let key = |v: &&VersionSummary| version_key(&semver::Version::parse(&v.version).unwrap());

        let latest = versions.iter().max_by_key(key).map(|v| v.version.clone());
        let mut fitting = Vec::new();
        for v in &versions {
            if self.fits(name, &v.version)? {
                fitting.push(v);
            }
        }
        let Some(best) = fitting
            .into_iter()
            .max_by_key(key)
            .map(|v| v.version.clone())
        else {
            if latest.is_none() || self.constraints(name).is_empty() {
                bail!("no published {name} for {target}; see `ortzi search`");
            }
            bail!(
                "{}: no version for {target} fits ({})",
                self.chain(name),
                self.describe(name)
            );
        };

        if root && let Some(latest) = latest.filter(|l| *l != best) {
            self.held.push(format!(
                "{name} {latest} held back by {}",
                self.describe(name)
            ));
        }
        Ok(best)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InstalledPackage;

    type Deps = Vec<(&'static str, &'static str)>;

    #[derive(Default)]
    struct Fake {
        packages: BTreeMap<&'static str, Vec<(&'static str, Deps, bool)>>,
    }

    impl Fake {
        fn add(
            mut self,
            name: &'static str,
            version: &'static str,
            deps: &[(&'static str, &'static str)],
        ) -> Self {
            self.packages
                .entry(name)
                .or_default()
                .push((version, deps.to_vec(), false));
            self
        }

        fn yank(mut self, name: &'static str, version: &'static str) -> Self {
            for v in self.packages.get_mut(name).unwrap() {
                if v.0 == version {
                    v.2 = true;
                }
            }
            self
        }
    }

    impl Catalog for Fake {
        type Release = ();

        async fn versions(&self, name: &str) -> anyhow::Result<Vec<VersionSummary>> {
            let Some(versions) = self.packages.get(name) else {
                bail!("package {name} not found");
            };
            Ok(versions
                .iter()
                .map(|(version, _, yanked)| VersionSummary {
                    version: (*version).to_owned(),
                    targets: vec![Target::LinuxAmd64],
                    published_at: None,
                    yanked: *yanked,
                })
                .collect())
        }

        async fn release(
            &self,
            name: &str,
            version: &str,
        ) -> anyhow::Result<((), BTreeMap<String, String>)> {
            let (_, deps, _) = self.packages[name]
                .iter()
                .find(|(v, _, _)| *v == version)
                .ok_or_else(|| anyhow::anyhow!("{name}@{version} not found"))?;
            Ok((
                (),
                deps.iter()
                    .map(|(n, r)| ((*n).to_owned(), (*r).to_owned()))
                    .collect(),
            ))
        }
    }

    fn catalog() -> Fake {
        Fake::default()
            .add("helm", "4.2.0", &[])
            .add("helm", "4.3.1", &[])
            .add("helm", "4.3.1-2", &[])
            .add("helm", "5.0.0", &[])
            .add("helm", "5.1.0-rc.1", &[])
            .add("kustomize", "5.7.1", &[])
            .add("app", "0.78.0", &[("helm", "^4.3"), ("kustomize", "^5.7")])
    }

    type Have<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], bool);

    fn installed(packages: &[Have<'_>]) -> Installed {
        let mut installed = Installed::default();
        for (name, version, deps, pinned) in packages {
            let package: InstalledPackage = serde_json::from_value(serde_json::json!({
                "version": version,
                "target": "linux-amd64",
                "sha256": "",
                "binary": name,
                "pinned": pinned,
                "dependencies": deps.iter().map(|(n, r)| (n.to_string(), r.to_string())).collect::<BTreeMap<_, _>>(),
            }))
            .unwrap();
            installed.packages.insert((*name).to_owned(), package);
        }
        installed
    }

    fn root(name: &str) -> Root<'_> {
        Root {
            name,
            version: None,
            requested: true,
        }
    }

    async fn plan(
        catalog: &Fake,
        installed: &Installed,
        roots: &[Root<'_>],
    ) -> anyhow::Result<Vec<String>> {
        let plan = resolve(catalog, installed, Target::LinuxAmd64, roots, false).await?;
        Ok(plan
            .steps
            .iter()
            .map(|s| {
                format!(
                    "{}@{}{}",
                    s.name,
                    s.version,
                    if s.requested { "" } else { " (dep)" }
                )
            })
            .collect())
    }

    #[tokio::test]
    async fn installs_dependencies_first_at_the_highest_fitting_version() {
        let steps = plan(&catalog(), &Installed::default(), &[root("app")])
            .await
            .unwrap();
        assert_eq!(
            steps,
            ["helm@4.3.1-2 (dep)", "kustomize@5.7.1 (dep)", "app@0.78.0"]
        );
    }

    #[tokio::test]
    async fn keeps_installed_dependencies_that_fit() {
        let installed = installed(&[("helm", "4.3.1", &[], false)]);
        let steps = plan(&catalog(), &installed, &[root("app")]).await.unwrap();
        assert_eq!(steps, ["kustomize@5.7.1 (dep)", "app@0.78.0"]);
    }

    #[tokio::test]
    async fn upgrades_installed_dependencies_that_do_not_fit() {
        let installed = installed(&[("helm", "4.2.0", &[], false)]);
        let steps = plan(&catalog(), &installed, &[root("app")]).await.unwrap();
        assert_eq!(steps[0], "helm@4.3.1-2 (dep)");
    }

    #[tokio::test]
    async fn pinned_dependencies_that_do_not_fit_fail() {
        let installed = installed(&[("helm", "4.2.0", &[], true)]);
        let error = plan(&catalog(), &installed, &[root("app")])
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("helm is pinned at 4.2.0 but app needs ^4.3"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn dependents_hold_back_updates() {
        let installed = installed(&[
            ("app", "0.78.0", &[("helm", "^4.3")], false),
            ("helm", "4.3.1", &[], false),
        ]);
        let roots = [Root {
            name: "helm",
            version: None,
            requested: false,
        }];
        let plan = resolve(&catalog(), &installed, Target::LinuxAmd64, &roots, false)
            .await
            .unwrap();
        assert_eq!(plan.steps[0].version, "4.3.1-2");
        assert_eq!(plan.held, ["helm 5.0.0 held back by app needs ^4.3"]);
    }

    #[tokio::test]
    async fn exact_versions_must_fit_dependents_unless_forced() {
        let installed = installed(&[("app", "0.78.0", &[("helm", "^4.3")], false)]);
        let roots = [Root {
            name: "helm",
            version: Some("5.0.0"),
            requested: true,
        }];
        assert!(plan(&catalog(), &installed, &roots).await.is_err());
        let forced = resolve(&catalog(), &installed, Target::LinuxAmd64, &roots, true)
            .await
            .unwrap();
        assert_eq!(forced.steps[0].version, "5.0.0");
    }

    #[tokio::test]
    async fn conflicting_ranges_fail_with_the_chain() {
        let catalog = catalog().add("other", "1.0.0", &[("helm", "^5")]);
        let error = plan(
            &catalog,
            &Installed::default(),
            &[root("app"), root("other")],
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("other → helm: no version for linux-amd64 fits"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn yanked_and_prerelease_versions_are_skipped() {
        let catalog = catalog().yank("helm", "4.3.1-2");
        let steps = plan(&catalog, &Installed::default(), &[root("helm")])
            .await
            .unwrap();
        assert_eq!(steps, ["helm@5.0.0"]);
        let steps = plan(&catalog, &Installed::default(), &[root("app")])
            .await
            .unwrap();
        assert_eq!(steps[0], "helm@4.3.1 (dep)");
    }

    #[tokio::test]
    async fn cycles_are_refused() {
        let catalog =
            Fake::default()
                .add("a", "1.0.0", &[("b", "*")])
                .add("b", "1.0.0", &[("a", "*")]);
        let error = plan(&catalog, &Installed::default(), &[root("a")])
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "dependency cycle: a → b → a");
    }

    #[tokio::test]
    async fn a_later_range_repicks_an_earlier_dependency() {
        let catalog = catalog().add("loose", "1.0.0", &[("helm", "*")]).add(
            "strict",
            "1.0.0",
            &[("helm", "~4.3.0")],
        );
        let steps = plan(
            &catalog,
            &Installed::default(),
            &[root("loose"), root("strict")],
        )
        .await
        .unwrap();
        assert_eq!(steps, ["helm@4.3.1-2 (dep)", "loose@1.0.0", "strict@1.0.0"]);
    }

    #[tokio::test]
    async fn requesting_a_dependency_marks_it_requested() {
        let steps = plan(
            &catalog(),
            &Installed::default(),
            &[root("app"), root("helm")],
        )
        .await
        .unwrap();
        assert!(steps.contains(&"helm@4.3.1-2".to_owned()), "{steps:?}");
    }
}
