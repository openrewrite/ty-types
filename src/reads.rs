use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::sync::{Arc, Mutex};

use ruff_db::system::walk_directory::WalkDirectoryBuilder;
use ruff_db::system::{
    CommandExecutor, DirectoryEntry, Metadata, OsSystem, Result, System, SystemPath, SystemPathBuf,
    SystemVirtualPath, WhichResult, WritableSystem,
};
use ruff_notebook::{Notebook, NotebookError};

/// The files a [`RecordingSystem`] has found and read, shared with every clone of it.
#[derive(Debug, Clone, Default)]
pub struct ReadFiles(Arc<Mutex<BTreeSet<SystemPathBuf>>>);

impl ReadFiles {
    pub fn to_vec(&self) -> Vec<String> {
        let read = self.0.lock().unwrap();
        read.iter().map(|path| path.to_string()).collect()
    }

    fn record(&self, path: &SystemPath) {
        let mut read = self.0.lock().unwrap();
        if !read.contains(path) {
            read.insert(path.to_path_buf());
        }
    }
}

/// An [`OsSystem`] that records the path of every file whose content it reads.
///
/// ty need not read a cached file again, so the set is complete only when recording
/// starts with the database.
#[derive(Debug, Clone)]
pub struct RecordingSystem {
    inner: OsSystem,
    read: ReadFiles,
}

impl RecordingSystem {
    pub fn new(inner: OsSystem) -> Self {
        Self {
            inner,
            read: ReadFiles::default(),
        }
    }

    pub fn read_files(&self) -> ReadFiles {
        self.read.clone()
    }

    /// Records `path` unless the read found no file there: ty tries a configuration
    /// file by reading it, so a missing one is not a file it read.
    fn record_unless_not_found(&self, path: &SystemPath, error: Option<&std::io::Error>) {
        if error.is_none_or(|error| error.kind() != ErrorKind::NotFound) {
            self.read.record(path);
        }
    }
}

impl System for RecordingSystem {
    fn path_metadata(&self, path: &SystemPath) -> Result<Metadata> {
        self.inner.path_metadata(path)
    }

    fn canonicalize_path(&self, path: &SystemPath) -> Result<SystemPathBuf> {
        self.inner.canonicalize_path(path)
    }

    fn is_same_file(&self, first: &SystemPath, second: &SystemPath) -> Result<bool> {
        self.inner.is_same_file(first, second)
    }

    fn path_exists(&self, path: &SystemPath) -> bool {
        self.inner.path_exists(path)
    }

    fn which(&self, binary_name: &str) -> WhichResult {
        self.inner.which(binary_name)
    }

    fn command_executor(&self) -> Option<&dyn CommandExecutor> {
        self.inner.command_executor()
    }

    fn read_to_string(&self, path: &SystemPath) -> Result<String> {
        let result = self.inner.read_to_string(path);
        self.record_unless_not_found(path, result.as_ref().err());
        result
    }

    fn read_to_notebook(&self, path: &SystemPath) -> std::result::Result<Notebook, NotebookError> {
        let result = self.inner.read_to_notebook(path);
        let io_error = match &result {
            Err(NotebookError::Io(error)) => Some(error),
            _ => None,
        };
        self.record_unless_not_found(path, io_error);
        result
    }

    fn read_virtual_path_to_string(&self, path: &SystemVirtualPath) -> Result<String> {
        self.inner.read_virtual_path_to_string(path)
    }

    fn read_virtual_path_to_notebook(
        &self,
        path: &SystemVirtualPath,
    ) -> std::result::Result<Notebook, NotebookError> {
        self.inner.read_virtual_path_to_notebook(path)
    }

    fn current_directory(&self) -> &SystemPath {
        self.inner.current_directory()
    }

    fn user_config_directory(&self) -> Option<SystemPathBuf> {
        self.inner.user_config_directory()
    }

    fn cache_dir(&self) -> Option<SystemPathBuf> {
        self.inner.cache_dir()
    }

    fn read_directory<'a>(
        &'a self,
        path: &SystemPath,
    ) -> Result<Box<dyn Iterator<Item = Result<DirectoryEntry>> + 'a>> {
        self.inner.read_directory(path)
    }

    fn walk_directory(&self, path: &SystemPath) -> WalkDirectoryBuilder {
        self.inner.walk_directory(path)
    }

    fn env_var(&self, name: &str) -> std::result::Result<String, std::env::VarError> {
        self.inner.env_var(name)
    }

    fn as_writable(&self) -> Option<&dyn WritableSystem> {
        Some(self)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn dyn_clone(&self) -> Box<dyn System> {
        Box::new(self.clone())
    }
}

impl WritableSystem for RecordingSystem {
    fn create_new_file(&self, path: &SystemPath) -> Result<()> {
        self.inner.create_new_file(path)
    }

    fn write_file_bytes(&self, path: &SystemPath, content: &[u8]) -> Result<()> {
        self.inner.write_file_bytes(path, content)
    }

    fn create_directory_all(&self, path: &SystemPath) -> Result<()> {
        self.inner.create_directory_all(path)
    }

    fn dyn_clone(&self) -> Box<dyn WritableSystem> {
        Box::new(self.clone())
    }
}
