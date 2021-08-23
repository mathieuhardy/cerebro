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

const MODULE_NAME: &str = "battery";

const VALUE_FALSE: &str = "false";
const VALUE_TRUE: &str = "true";
const VALUE_UNKNOWN: &str = "?";

const ENTRY_PERCENT: &str = "percent";
const ENTRY_PLUGGED: &str = "plugged";
const ENTRY_TIME_REMAINING: &str = "time_remaining";

/// Information about the battery
#[derive(Serialize)]
struct BatteryData
{
    pub plugged: String,
    pub percent: String,
    pub time_remaining: String,
}

impl BatteryData {
    /// BatteryData constructor
    pub fn new() -> Self {
        Self {
            plugged: VALUE_UNKNOWN.to_string(),
            percent: VALUE_UNKNOWN.to_string(),
            time_remaining: VALUE_UNKNOWN.to_string(),
        }
    }
}

/// Battery backend that will compute the values
struct BatteryBackend {
    system_stats: systemstat::System,
    triggers: Vec<triggers::Trigger>,

    pub data: BatteryData,
}

impl BatteryBackend {
    fn new(triggers: &Vec<triggers::Trigger>) -> Self {
        Self {
            system_stats: systemstat::System::new(),
            triggers: triggers.to_vec(),
            data: BatteryData::new(),
        }
    }
}

impl module::Data for BatteryBackend {
    /// Update battery data
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn update(&mut self) -> Result<module::Status, error::CerebroError> {
        // Plugged status
        let plugged = match self.system_stats.on_ac_power() {
            Ok(power) => match power {
                true => VALUE_TRUE.to_string(),
                false => VALUE_FALSE.to_string(),
            },

            Err(_) => VALUE_UNKNOWN.to_string(),
        };

        if plugged != self.data.plugged {
            let old_value = self.data.plugged.clone();

            self.data.plugged = plugged;

            log::debug!("{}: plugged={}", MODULE_NAME, self.data.plugged);

            triggers::find_all_and_execute(
                &self.triggers,
                triggers::Kind::Update,
                MODULE_NAME,
                ENTRY_PLUGGED,
                &old_value,
                &self.data.plugged);
        }

        // Percent and time remaining
        let (percent, time_remaining) = match self.system_stats.battery_life() {
            Ok(battery) => {
                let capacity = battery.remaining_capacity;
                let time = battery.remaining_time.as_secs();

                (
                    ((capacity * 100.0).ceil() as u8).to_string(),
                    format!("{:0>2}h{:0>2}m", time / 3600, time % 60)
                )
            },

            Err(_) => (VALUE_UNKNOWN.to_string(), VALUE_UNKNOWN.to_string()),
        };

        if percent != self.data.percent {
            let old_value = self.data.percent.clone();

            self.data.percent = percent;

            log::debug!("{}: percent={}", MODULE_NAME, self.data.percent);

            triggers::find_all_and_execute(
                &self.triggers,
                triggers::Kind::Update,
                MODULE_NAME,
                ENTRY_PERCENT,
                &old_value,
                &self.data.percent);
        }

        if time_remaining != self.data.time_remaining {
            let old_value = self.data.time_remaining.clone();

            self.data.time_remaining = time_remaining;

            log::debug!(
                "{}: time_remaining={}",
                MODULE_NAME,
                self.data.time_remaining);

            triggers::find_all_and_execute(
                &self.triggers,
                triggers::Kind::Update,
                MODULE_NAME,
                ENTRY_TIME_REMAINING,
                &old_value,
                &self.data.time_remaining);
        }

        return Ok(module::Status::Ok);
    }
}

/// Battery module structure
pub struct Battery {
    thread: Arc<Mutex<module::Thread>>,
    inode_plugged: u64,
    inode_percent: u64,
    inode_time_remaining: u64,
    backend: Arc<Mutex<BatteryBackend>>,
    fs_entries: Vec<filesystem::FsEntry>,
}

impl Battery {
    /// Battery constructor
    pub fn new(
        event_manager: &mut event_manager::EventManager,
        triggers: &Vec<triggers::Trigger>) -> Self {

        let plugged = filesystem::FsEntry::create_inode();
        let percent = filesystem::FsEntry::create_inode();
        let time_remaining = filesystem::FsEntry::create_inode();

        Self {
            thread: Arc::new(Mutex::new(
                module::Thread::new(event_manager.sender()))),

            inode_plugged: plugged,
            inode_percent: percent,
            inode_time_remaining: time_remaining,
            backend: Arc::new(Mutex::new(BatteryBackend::new(triggers))),
            fs_entries: vec![
                filesystem::FsEntry::new(
                    plugged,
                    fuse::FileType::RegularFile,
                    ENTRY_PLUGGED,
                    filesystem::Mode::ReadOnly,
                    &Vec::new()),

                filesystem::FsEntry::new(
                    percent,
                    fuse::FileType::RegularFile,
                    ENTRY_PERCENT,
                    filesystem::Mode::ReadOnly,
                    &Vec::new()),

                filesystem::FsEntry::new(
                    time_remaining,
                    fuse::FileType::RegularFile,
                    ENTRY_TIME_REMAINING,
                    filesystem::Mode::ReadOnly,
                    &Vec::new()),
                ],
        }
    }
}

impl module::Module for Battery {
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

        thread.start(self.backend.clone(), config.timeout_s)?;

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
        if inode == self.inode_percent {
            match self.backend.lock() {
                Ok(b) => return b.data.percent.clone(),
                Err(_) => return VALUE_UNKNOWN.to_string(),
            }
        }

        if inode == self.inode_plugged {
            match self.backend.lock() {
                Ok(b) => return b.data.plugged.clone(),
                Err(_) => return VALUE_UNKNOWN.to_string(),
            }
        }

        if inode == self.inode_time_remaining {
            match self.backend.lock() {
                Ok(b) => return b.data.time_remaining.clone(),
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
            "plugged={} percent={} time_remaining={}",
            backend.data.plugged,
            backend.data.percent,
            backend.data.time_remaining).to_string();
    }
}
