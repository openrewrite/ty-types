use anyhow::Context;
use ruff_db::system::{OsSystem, SystemPath, SystemPathBuf};
use ty_project::{ProjectDatabase, ProjectMetadata};

pub fn create_database(project_root: &str) -> anyhow::Result<ProjectDatabase> {
    let path = SystemPathBuf::from_path_buf(std::path::PathBuf::from(project_root))
        .map_err(|p| anyhow::anyhow!("Non-Unicode path: {}", p.display()))?;

    let system = OsSystem::new(&path);
    let system_path = SystemPath::new(path.as_str());

    let mut metadata = ProjectMetadata::discover(system_path, &system)
        .context("Failed to discover project metadata")?;

    metadata
        .apply_configuration_files(&system)
        .context("Failed to apply configuration files")?;

    ProjectDatabase::new(metadata, system).context("Failed to create project database")
}
