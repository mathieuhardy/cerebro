use fuse;
use serde::{Serialize};
use std::sync::{Arc, Mutex};
use systemstat::Platform;

use crate::config;
use crate::error;
use crate::event_manager;
use crate::filesystem;
use crate::modules::module;
use crate::triggers;

const MODULE_NAME: &str = "memory";

const VALUE_UNKNOWN: &str = "?";

const ENTRY_FREE: &str = "free";
const ENTRY_TOTAL: &str = "total";
const ENTRY_USED: &str = "used";

/// Information about the memory
#[derive(Serialize)]
struct MemoryData
{
    pub free: String,
    pub total: String,
    pub used: String,
}

impl MemoryData {
    /// MemoryData constructor
    pub fn new() -> Self {
        Self {
            free: VALUE_UNKNOWN.to_string(),
            total: VALUE_UNKNOWN.to_string(),
            used: VALUE_UNKNOWN.to_string(),
        }
    }
}

/// Memory backend that will compute the values
struct MemoryBackend {
    system_stats: systemstat::System,
    triggers: Vec<triggers::Trigger>,
    first_update: bool,

    pub data: MemoryData,
}

impl MemoryBackend {
    fn new(triggers: &Vec<triggers::Trigger>) -> Self {
        Self {
            system_stats: systemstat::System::new(),
            triggers: triggers.to_vec(),
            first_update: true,
            data: MemoryData::new(),
        }
    }
}

impl module::Data for MemoryBackend {
    /// Update memory data
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn update(&mut self) -> Result<module::Status, error::CerebroError> {
        let kind = match self.first_update {
            true => triggers::Kind::Create,
            false => triggers::Kind::Update,
        };

        let memory = match self.system_stats.memory() {
            Ok(m) => m,
            Err(_) => return error!("Cannot get memory statistics"),
        };

        let free = format!("{}", memory.free.as_u64());
        let total = format!("{}", memory.total.as_u64());
        let used = format!("{}", memory.total.as_u64() - memory.free.as_u64());

        // Free status
        if free != self.data.free {
            let old_value = self.data.free.clone();

            self.data.free = free;

            log::debug!("{}: free={}", MODULE_NAME, self.data.free);

            triggers::find_all_and_execute(
                &self.triggers,
                kind,
                MODULE_NAME,
                ENTRY_FREE,
                &old_value,
                &self.data.free);
        }

        // Total status
        if total != self.data.total {
            let old_value = self.data.total.clone();

            self.data.total = total;

            log::debug!("{}: total={}", MODULE_NAME, self.data.total);

            triggers::find_all_and_execute(
                &self.triggers,
                kind,
                MODULE_NAME,
                ENTRY_TOTAL,
                &old_value,
                &self.data.total);
        }

        // Used status
        if used != self.data.used {
            let old_value = self.data.used.clone();

            self.data.used = used;

            log::debug!("{}: used={}", MODULE_NAME, self.data.used);

            triggers::find_all_and_execute(
                &self.triggers,
                kind,
                MODULE_NAME,
                ENTRY_USED,
                &old_value,
                &self.data.used);
        }

        self.first_update = false;

        return Ok(module::Status::Ok);
    }
}

/// Memory module structure
pub struct Memory {
    thread: Arc<Mutex<module::Thread>>,
    inode_free: u64,
    inode_total: u64,
    inode_used: u64,
    backend: Arc<Mutex<MemoryBackend>>,
    fs_entries: Vec<filesystem::FsEntry>,
}

impl Memory {
    /// Memory constructor
    pub fn new(
        event_manager: &mut event_manager::EventManager,
        triggers: &Vec<triggers::Trigger>) -> Self {

        let free = filesystem::FsEntry::create_inode();
        let total = filesystem::FsEntry::create_inode();
        let used = filesystem::FsEntry::create_inode();

        Self {
            thread: Arc::new(Mutex::new(
                module::Thread::new(event_manager.sender()))),

            inode_free: free,
            inode_total: total,
            inode_used: used,
            backend: Arc::new(Mutex::new(MemoryBackend::new(triggers))),
            fs_entries: vec![
                filesystem::FsEntry::new(
                    free,
                    fuse::FileType::RegularFile,
                    ENTRY_FREE,
                    filesystem::Mode::ReadOnly,
                    &Vec::new()),

                filesystem::FsEntry::new(
                    total,
                    fuse::FileType::RegularFile,
                    ENTRY_TOTAL,
                    filesystem::Mode::ReadOnly,
                    &Vec::new()),

                filesystem::FsEntry::new(
                    used,
                    fuse::FileType::RegularFile,
                    ENTRY_USED,
                    filesystem::Mode::ReadOnly,
                    &Vec::new()),
                ],
        }
    }
}

impl module::Module for Memory {
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
    fn start(&mut self, config: &config::ModuleConfig) -> error::Return {
        let mut thread = match self.thread.lock() {
            Ok(t) => t,
            Err(_) => return error!("Cannot lock thread"),
        };

        thread.start(self.backend.clone(), config.timeout_s)?;

        return success!();
    }

    /// Stop the module
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn stop(&mut self) -> error::Return {
        let mut thread = match self.thread.lock() {
            Ok(t) => t,
            Err(_) => return error!("Cannot lock thread"),
        };

        thread.stop()?;

        return success!();
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
        if inode == self.inode_free {
            match self.backend.lock() {
                Ok(b) => return b.data.free.clone(),
                Err(_) => return VALUE_UNKNOWN.to_string(),
            }
        }

        if inode == self.inode_total {
            match self.backend.lock() {
                Ok(b) => return b.data.total.clone(),
                Err(_) => return VALUE_UNKNOWN.to_string(),
            }
        }

        if inode == self.inode_used {
            match self.backend.lock() {
                Ok(b) => return b.data.used.clone(),
                Err(_) => return VALUE_UNKNOWN.to_string(),
            }
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
    fn set_value(&mut self, _inode: u64, _data: &[u8]) {
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

        return format!(
            "free={} total={} used={}",
            backend.data.free,
            backend.data.total,
            backend.data.used).to_string();
    }
}
