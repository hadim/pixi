use std::{hash::Hash, path::PathBuf};

use itertools::Itertools;
use pixi_manifest as manifest;
use pixi_manifest::{
    FeaturesExt, HasFeaturesIter, HasWorkspaceManifest, SystemRequirements, WorkspaceManifest,
};

use super::{Environment, HasWorkspaceRef, Workspace};

/// A grouping of environments that are solved together.
#[derive(Debug, Clone)]
pub struct SolveGroup<'p> {
    /// The project that the group is part of.
    pub(super) workspace: &'p Workspace,

    /// A reference to the solve group in the manifest
    pub(super) solve_group: &'p manifest::SolveGroup,
}

impl PartialEq<Self> for SolveGroup<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.solve_group, other.solve_group)
            && std::ptr::eq(self.workspace, other.workspace)
    }
}

impl Eq for SolveGroup<'_> {}

impl Hash for SolveGroup<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(self.solve_group, state);
        std::ptr::hash(self.workspace, state);
    }
}

impl<'p> SolveGroup<'p> {
    /// The name of the group
    pub(crate) fn name(&self) -> &str {
        &self.solve_group.name
    }

    /// Returns the directory where this solve group stores its environment
    pub(crate) fn dir(&self) -> PathBuf {
        self.workspace
            .solve_group_environments_dir()
            .join(self.name())
    }

    /// Returns an iterator over all the environments that are part of the
    /// group.
    pub(crate) fn environments(
        &self,
    ) -> impl DoubleEndedIterator<Item = Environment<'p>> + ExactSizeIterator + 'p {
        let workspace_manifest = self.workspace_manifest();
        self.solve_group.environments.iter().map(|env_idx| {
            Environment::new(self.workspace, &workspace_manifest.environments[*env_idx])
        })
    }
    /// Returns the system requirements for this solve group.
    ///
    /// The system requirements of the solve group are the union of the system
    /// requirements of all the environments that share the same solve
    /// group. If multiple environments specify a requirement for the same
    /// system package, the highest is chosen.
    pub(crate) fn system_requirements(&self) -> SystemRequirements {
        self.local_system_requirements()
    }
}

impl<'p> HasWorkspaceManifest<'p> for SolveGroup<'p> {
    fn workspace_manifest(&self) -> &'p WorkspaceManifest {
        self.workspace.workspace_manifest()
    }
}

impl<'p> HasFeaturesIter<'p> for SolveGroup<'p> {
    /// Returns all features that are part of the solve group.
    ///
    /// All features of all environments are combined and deduplicated.
    fn features(&self) -> impl DoubleEndedIterator<Item = &'p manifest::Feature> + 'p {
        self.environments()
            .flat_map(|env: Environment<'p>| env.features().collect_vec().into_iter())
            .unique_by(|feat| &feat.name)
    }
}

impl<'p> HasWorkspaceRef<'p> for SolveGroup<'p> {
    fn workspace(&self) -> &'p Workspace {
        self.workspace
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, path::Path};

    use itertools::Itertools;
    use pixi_manifest::FeaturesExt;
    use rattler_conda_types::PackageName;

    use crate::Workspace;

    #[test]
    fn test_solve_group() {
        let project = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
        [project]
        name = "foobar"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64"]

        [dependencies]
        a = "*"

        [feature.foo.dependencies]
        b = "*"

        [feature.foo.pypi-options]
        index-url = "https://my-index.com/simple"

        [feature.bar.dependencies]
        c = "*"

        [feature.bar.system-requirements]
        cuda = "12.0"

        [environments]
        foo = { features=["foo"], solve-group="group1" }
        bar = { features=["bar"], solve-group="group1" }
        baz = { features=["bar"], solve-group="group2", no-default-feature=true }
        "#,
        )
        .unwrap();

        let environments = project.environments();
        assert_eq!(environments.len(), 4);

        let default_environment = project.default_environment();
        let foo_environment = project.environment("foo").unwrap();
        let bar_environment = project.environment("bar").unwrap();

        let solve_groups = project.solve_groups();
        assert_eq!(solve_groups.len(), 2);

        let solve_group = solve_groups[0].clone();
        let solve_group_envs = solve_group.environments().collect_vec();
        assert_eq!(solve_group_envs.len(), 2);
        assert_eq!(solve_group_envs[0].name(), "foo");
        assert_eq!(solve_group_envs[1].name(), "bar");

        // Make sure that the environments properly reference the group
        assert_eq!(foo_environment.solve_group(), Some(solve_group.clone()));
        assert_eq!(bar_environment.solve_group(), Some(solve_group.clone()));
        assert_eq!(default_environment.solve_group(), None);

        // Make sure that all the environments share the same system requirements,
        // because they are in the same solve-group.
        let foo_system_requirements = foo_environment.system_requirements();
        let bar_system_requirements = bar_environment.system_requirements();
        let default_system_requirements = default_environment.system_requirements();
        assert_eq!(foo_system_requirements.cuda, "12.0".parse().ok());
        assert_eq!(bar_system_requirements.cuda, "12.0".parse().ok());
        assert_eq!(default_system_requirements.cuda, None);

        assert_eq!(
            solve_group.pypi_options().index_url.unwrap(),
            "https://my-index.com/simple".parse().unwrap()
        );

        // Check that the solve group 'group1' contains all the dependencies of its
        // environments
        let package_names: HashSet<_> = solve_group
            .combined_dependencies(None)
            .names()
            .cloned()
            .collect();
        assert_eq!(
            package_names,
            ["a", "b", "c"]
                .into_iter()
                .map(PackageName::new_unchecked)
                .collect::<HashSet<_>>()
        );

        // Check that the solve group 'group2' contains all the dependencies of its
        // environments it should not contain 'a', which is a dependency of the
        // default environment
        let solve_group = solve_groups[1].clone();
        let package_names: HashSet<_> = solve_group
            .combined_dependencies(None)
            .names()
            .cloned()
            .collect();
        assert_eq!(
            package_names,
            ["c"]
                .into_iter()
                .map(PackageName::new_unchecked)
                .collect::<HashSet<_>>()
        );
    }
}
