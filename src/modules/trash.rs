use dirs;
use fuse;
use notify::Watcher;
use serde::{Serialize};
use std::fs;
use std::io;
use std::path;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use walkdir;

use crate::config;
use crate::error;
use crate::event_manager;
use crate::filesystem;
use crate::modules::module;
use crate::triggers;

const MODULE_NAME: &str = "trash";

const VALUE_UNKNOWN: &str = "?";

const ENTRY_COUNT: &str = "count";
const ENTRY_EMPTY: &str = "empty";

/// Information about the trash
#[derive(Serialize)]
struct TrashData
{
    pub count: String,
}

impl TrashData {
    /// TrashData constructor
    pub fn new() -> Self {
        Self {
            count: VALUE_UNKNOWN.to_string(),
        }
    }
}

/// Proxy backend that is only use in the context of the thread
struct TrashBackendProxy {
    backend: Arc<Mutex<TrashBackend>>,
}

impl TrashBackendProxy {
    fn new(backend: Arc<Mutex<TrashBackend>>) -> Self {
        Self {
            backend: backend,
        }
    }

    fn update_count(&mut self) -> error::CerebroResult{
        let home_dir = match dirs::home_dir() {
            Some(path) => path,
            None => return error!("Cannot get home directory"),
        };

        let path = home_dir
            .join(".local")
            .join("share")
            .join("Trash")
            .join("files");

        // Fetch number of files in directory
        let count = format!(
            "{}",
            walkdir::WalkDir::new(&path).into_iter().count() - 1);

        // Lock backend
        let mut backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => return error!("Cannot lock backend"),
        };

        if count != backend.data.count {
            let old_value = backend.data.count.clone();

            backend.data.count = count;

            log::debug!("{}: count={}", MODULE_NAME, backend.data.count);

            triggers::find_all_and_execute(
                &backend.triggers,
                triggers::Kind::Update,
                MODULE_NAME,
                ENTRY_COUNT,
                &old_value,
                &backend.data.count);
        }

        return Success!();
    }
}

impl module::Data for TrashBackendProxy {
    /// Update trash data
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn update(&mut self) -> Result<module::Status, error::CerebroError> {
        let home_dir = match dirs::home_dir() {
            Some(path) => path,
            None => return error!("Cannot get home directory"),
        };

        let watch_path = home_dir.join(".local").join("share").join("Trash");

        // Create watcher
        let (tx, rx) = mpsc::channel();

        let mut w: notify::INotifyWatcher = match notify::Watcher::new_raw(tx) {
            Ok(w) => w,
            Err(_) => return error!("Cannot create filesystem watcher"),
        };

        // Add watch paths
        match w.watch(watch_path, notify::RecursiveMode::Recursive) {
            Ok(_) => (),
            Err(_) => return error!("Cannot add path to watch"),
        }

        // Wait for events
        self.update_count()?;

        loop {
            let event = match rx.recv() {
                Ok(e) => e,
                Err(_) => return error!("Error during watching filesystem"),
            };

            let op = match event.op {
                Ok(o) => o,
                Err(_) => return error!("Watch event returned an error"),
            };

            match op {
                notify::Op::CREATE | notify::Op::REMOVE => (),
                _ => continue,
            }

            self.update_count()?;
        }
    }
}

/// Trash backend that will compute the values
struct TrashBackend {
    triggers: Vec<triggers::Trigger>,

    pub data: TrashData,
}

impl TrashBackend {
    fn new(triggers: &Vec<triggers::Trigger>) -> Self {
        Self {
            triggers: triggers.to_vec(),
            data: TrashData::new(),
        }
    }
}

/// Trash module structure
pub struct Trash {
    thread: Arc<Mutex<module::Thread>>,
    inode_count: u64,
    inode_empty: u64,
    backend: Arc<Mutex<TrashBackend>>,
    backend_proxy: Arc<Mutex<TrashBackendProxy>>,
    fs_entries: Vec<filesystem::FsEntry>,
}

impl Trash {
    /// Trash constructor
    pub fn new(
        event_manager: &mut event_manager::EventManager,
        triggers: &Vec<triggers::Trigger>) -> Self {

        let count = filesystem::FsEntry::create_inode();
        let empty = filesystem::FsEntry::create_inode();
        let backend = Arc::new(Mutex::new(TrashBackend::new(triggers)));

        Self {
            thread: Arc::new(Mutex::new(
                module::Thread::new(event_manager.sender()))),

            inode_count: count,
            inode_empty: empty,
            backend: backend.clone(),
            backend_proxy:
                Arc::new(Mutex::new(TrashBackendProxy::new(backend.clone()))),
            fs_entries: vec![
                filesystem::FsEntry::new(
                    count,
                    fuse::FileType::RegularFile,
                    ENTRY_COUNT,
                    filesystem::Mode::ReadOnly,
                    &Vec::new()),

                filesystem::FsEntry::new(
                    empty,
                    fuse::FileType::RegularFile,
                    ENTRY_EMPTY,
                    filesystem::Mode::WriteOnly,
                    &Vec::new())
                ],
        }
    }

    fn remove_dir_contents<P: AsRef<path::Path>>(path: P) -> io::Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();

            if entry.file_type()?.is_dir() {
                Trash::remove_dir_contents(&path)?;
                fs::remove_dir(path)?;
            } else {
                fs::remove_file(path)?;
            }
        }

        return Ok(());
    }
}

impl module::Module for Trash {
    /// Get name of the module
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn name(&self) -> &str {
        return MODULE_NAME;
    }

    /// Start the module
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn start(&mut self, config: &config::ModuleConfig) -> error::CerebroResult {
        let mut thread = match self.thread.lock() {
            Ok(t) => t,
            Err(_) => return error!("Cannot lock thread"),
        };

        thread.start(self.backend_proxy.clone(), config.timeout_s)?;

        return Success!();
    }

    /// Stop the module
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn stop(&mut self) -> error::CerebroResult {
        let mut thread = match self.thread.lock() {
            Ok(t) => t,
            Err(_) => return error!("Cannot lock thread"),
        };

        thread.stop()?;

        return Success!();
    }

    /// Check if module is running
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn is_running(&self) -> bool {
        let thread = match self.thread.lock() {
            Ok(t) => t,
            Err(_) => return false,
        };

        return thread.is_running();
    }

    /// Get filesystem entries of the module
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn fs_entries(&self) -> Vec<filesystem::FsEntry> {
        return self.fs_entries.to_vec();
    }

    /// Get value to be displayed for a filesystem entry
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `inode` - The inode of the filesystem to be fetched
    fn value(&self, inode: u64) -> String {
        if inode == self.inode_count {
            match self.backend.lock() {
                Ok(b) => return b.data.count.clone(),
                Err(_) => return VALUE_UNKNOWN.to_string(),
            }
        }

        if inode == self.inode_empty {
            return "".to_string();
        }

        return VALUE_UNKNOWN.to_string();
    }

    /// Set value of a filesystem entry
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `inode` - The inode of the filesystem to be written
    /// * `data` - The data to be written
    fn set_value(&mut self, inode: u64, data: &[u8]) {
        if inode == self.inode_empty {
            match data {
                b"1" | b"1\n" | b"true" | b"true\n" => {
                    let _backend = match self.backend.lock() {
                        Ok(b) => b,
                        Err(_) => {
                            println!("Cannot lock backend");
                            return;
                        },
                    };

                    let home_dir = match dirs::home_dir() {
                        Some(path) => path,
                        None => {
                            println!("Cannot get home directory");
                            return;
                        },
                    };

                    let trash_dir = home_dir
                        .join(".local")
                        .join("share")
                        .join("Trash");

                    let dir = trash_dir.join("files");

                    match Trash::remove_dir_contents(&dir) {
                        Ok(_) => (),
                        Err(_) => println!("Cannot empty directory: {:?}", dir),
                    }

                    let dir = trash_dir.join("info");

                    match Trash::remove_dir_contents(&dir) {
                        Ok(_) => (),
                        Err(_) => println!("Cannot empty directory: {:?}", dir),
                    }
                },

                _ => (),
            }
        }
    }

    /// Get value to be displayed for a filesystem entry (in JSON format)
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn json(&self) -> String {
        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => return VALUE_UNKNOWN.to_string(),
        };

        return match serde_json::to_string(&backend.data) {
            Ok(json) => json,
            Err(_) => VALUE_UNKNOWN.to_string(),
        }
    }

    /// Get value to be displayed for a filesystem entry (in shell format)
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn shell(&self) -> String {
        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => return VALUE_UNKNOWN.to_string(),
        };

        return format!("count={}", backend.data.count).to_string();
    }
}
