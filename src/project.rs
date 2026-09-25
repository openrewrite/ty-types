use anyhow::Context;
use ruff_db::system::{OsSystem, System, SystemPath, SystemPathBuf};
use ty_project::{ProjectDatabase, ProjectMetadata};

use crate::reads::{ReadFiles, RecordingSystem};

/// Creates the database for `project_root`, and returns the root it was built for and the
/// files the database reads.
///
/// The module resolver matches a file against canonicalized search roots, so a project
/// root spelled through a symlink matches none of them: the files under it belong to no
/// module, and the types declared in them are named from no root at all.
pub fn create_database(
    project_root: &str,
) -> anyhow::Result<(ProjectDatabase, SystemPathBuf, ReadFiles)> {
    let path = SystemPathBuf::from_path_buf(std::path::PathBuf::from(project_root))
        .map_err(|p| anyhow::anyhow!("Non-Unicode path: {}", p.display()))?;

    let system = RecordingSystem::new(OsSystem::new(&path));
    let read_files = system.read_files();
    let root = canonical(&system, &path);

    let mut metadata = ProjectMetadata::discover(SystemPath::new(root.as_str()), &system)
        .context("Failed to discover project metadata")?;

    metadata
        .apply_configuration_files(&system)
        .context("Failed to apply configuration files")?;

    let db =
        ProjectDatabase::fallible(metadata, system).context("Failed to create project database")?;

    Ok((db, root, read_files))
}

/// Resolves symlinks in `path` as the module resolver resolves them in a search root,
/// keeping it as given when it cannot be resolved.
pub fn canonical(system: &dyn System, path: &SystemPath) -> SystemPathBuf {
    system
        .canonicalize_path(path)
        .unwrap_or_else(|_| path.to_path_buf())
}
