use std::collections::BTreeMap;
use std::path::Path;

use uv_configuration::{LowerBound, SourceStrategy};
use uv_distribution_types::IndexLocations;
use uv_normalize::PackageName;
use uv_workspace::pyproject::ToolUvSources;
use uv_workspace::{DiscoveryOptions, ProjectWorkspace};

use crate::metadata::{GitWorkspaceMember, LoweredRequirement, MetadataError};

/// Lowered requirements from a `[build-system.requires]` field in a `pyproject.toml` file.
#[derive(Debug, Clone)]
pub struct BuildRequires {
    pub name: PackageName,
    pub requires_dist: Vec<uv_pypi_types::Requirement>,
}

impl BuildRequires {
    /// Lower without considering `tool.uv` in `pyproject.toml`, used for index and other archive
    /// dependencies.
    pub fn from_metadata23(metadata: uv_pypi_types::BuildRequires) -> Self {
        Self {
            name: metadata.name,
            requires_dist: metadata
                .requires_dist
                .into_iter()
                .map(uv_pypi_types::Requirement::from)
                .collect(),
        }
    }

    /// Lower by considering `tool.uv` in `pyproject.toml` if present, used for Git and directory
    /// dependencies.
    pub async fn from_project_maybe_workspace(
        metadata: uv_pypi_types::BuildRequires,
        install_path: &Path,
        git_member: Option<&GitWorkspaceMember<'_>>,
        locations: &IndexLocations,
        sources: SourceStrategy,
        lower_bound: LowerBound,
    ) -> Result<Self, MetadataError> {
        // TODO(konsti): Cache workspace discovery.
        let discovery_options = if let Some(git_member) = &git_member {
            DiscoveryOptions {
                stop_discovery_at: Some(
                    git_member
                        .fetch_root
                        .parent()
                        .expect("git checkout has a parent"),
                ),
                ..Default::default()
            }
        } else {
            DiscoveryOptions::default()
        };
        let Some(project_workspace) =
            ProjectWorkspace::from_maybe_project_root(install_path, &discovery_options).await?
        else {
            return Ok(Self::from_metadata23(metadata));
        };

        Self::from_project_workspace(
            metadata,
            &project_workspace,
            git_member,
            locations,
            sources,
            lower_bound,
        )
    }

    /// Lower the `build-system.requires` field from a `pyproject.toml` file.
    fn from_project_workspace(
        metadata: uv_pypi_types::BuildRequires,
        project_workspace: &ProjectWorkspace,
        git_member: Option<&GitWorkspaceMember<'_>>,
        locations: &IndexLocations,
        source_strategy: SourceStrategy,
        lower_bound: LowerBound,
    ) -> Result<Self, MetadataError> {
        // Collect any `tool.uv.index` entries.
        let empty = vec![];
        let project_indexes = match source_strategy {
            SourceStrategy::Enabled => project_workspace
                .current_project()
                .pyproject_toml()
                .tool
                .as_ref()
                .and_then(|tool| tool.uv.as_ref())
                .and_then(|uv| uv.index.as_deref())
                .unwrap_or(&empty),
            SourceStrategy::Disabled => &empty,
        };

        // Collect any `tool.uv.sources` and `tool.uv.dev_dependencies` from `pyproject.toml`.
        let empty = BTreeMap::default();
        let project_sources = match source_strategy {
            SourceStrategy::Enabled => project_workspace
                .current_project()
                .pyproject_toml()
                .tool
                .as_ref()
                .and_then(|tool| tool.uv.as_ref())
                .and_then(|uv| uv.sources.as_ref())
                .map(ToolUvSources::inner)
                .unwrap_or(&empty),
            SourceStrategy::Disabled => &empty,
        };

        // Lower the requirements.
        let requires_dist = metadata.requires_dist.into_iter();
        let requires_dist = match source_strategy {
            SourceStrategy::Enabled => requires_dist
                .flat_map(|requirement| {
                    let requirement_name = requirement.name.clone();
                    let extra = requirement.marker.top_level_extra_name();
                    let group = None;
                    LoweredRequirement::from_requirement(
                        requirement,
                        &metadata.name,
                        project_workspace.project_root(),
                        project_sources,
                        project_indexes,
                        extra.as_ref(),
                        group,
                        locations,
                        project_workspace.workspace(),
                        lower_bound,
                        git_member,
                    )
                    .map(move |requirement| match requirement {
                        Ok(requirement) => Ok(requirement.into_inner()),
                        Err(err) => Err(MetadataError::LoweringError(
                            requirement_name.clone(),
                            Box::new(err),
                        )),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            SourceStrategy::Disabled => requires_dist
                .into_iter()
                .map(uv_pypi_types::Requirement::from)
                .collect(),
        };

        Ok(Self {
            name: metadata.name,
            requires_dist,
        })
    }
}