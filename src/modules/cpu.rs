use fuse;
use regex::Regex;
use sensors::{FeatureType, Sensors, SubfeatureType};
use serde::{Serialize};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use systemstat::{CPULoad, DelayedMeasurement, Platform};

use crate::config;
use crate::error;
use crate::event_manager;
use crate::filesystem;
use crate::modules::module;
use crate::triggers;

const MODULE_NAME: &str = "cpu";

const ENTRY_AVERRAGE: &str = "averrage";
const ENTRY_COUNT: &str = "count";
const ENTRY_LOGICAL: &str = "logical";
const ENTRY_PHYSICAL: &str = "physical";
const ENTRY_TEMPERATURE: &str = "temperature";
const ENTRY_TIMESTAMP: &str = "timestamp";
const ENTRY_USAGE: &str = "usage_percent";

const VALUE_UNKNOWN: &str = "?";

/// Information of one logical CPU
#[derive(Debug, PartialEq, Serialize)]
struct LogicalData {
    pub usage_percent: String,
}

impl LogicalData {
    /// LogicalData constructor
    pub fn new(usage: f32) -> Self {
        Self {
            usage_percent: format!("{}", usage * 100f32),
        }
    }
}

/// Information of one physical CPU
#[derive(Debug, PartialEq, Serialize)]
struct PhysicalData {
    pub temperature: String,
}

impl PhysicalData {
    /// PhysicalData constructor
    pub fn new(temperature: i16) -> Self {
        Self {
            temperature: match temperature {
                t if t >= 0 => format!("{}", temperature),
                _ => VALUE_UNKNOWN.to_string(),
            }
        }
    }
}

/// Information about the list of CPU
#[derive(Serialize)]
struct CpuListData {
    pub logical_timestamp: String,
    pub logical_averrage_usage: String,
    pub logical_count: String,
    pub logical_list: Vec<LogicalData>,

    pub physical_timestamp: String,
    pub physical_count: String,
    pub physical_list: Vec<PhysicalData>,
}

impl CpuListData {
    /// CpuListData constructor
    pub fn new() -> Self {
        Self {
            logical_timestamp: "0".to_string(),
            logical_count: "0".to_string(),
            logical_averrage_usage: "0".to_string(),
            logical_list: Vec::new(),
            physical_timestamp: "0".to_string(),
            physical_count: "0".to_string(),
            physical_list: Vec::new(),
        }
    }
}

/// CPU backend that will compute the values
struct CpuBackend {
    config: config::ModuleConfig,
    system_stats: systemstat::System,
    cpu_stats: Option<DelayedMeasurement<Vec<CPULoad>>>,
    triggers: Vec<triggers::Trigger>,

    pub inode_logical_timestamp: u64,
    pub inode_physical_timestamp: u64,
    pub inode_logical_averrage: u64,
    pub inode_logical_averrage_usage: u64,
    pub inode_logical_count: u64,
    pub inode_physical_count: u64,
    pub data: CpuListData,
    pub static_fs_entries: Vec<filesystem::FsEntry>,
    pub logical_fs_entries: Vec<filesystem::FsEntry>,
    pub physical_fs_entries: Vec<filesystem::FsEntry>,
}

impl CpuBackend {
    /// CpuBackend constructor
    fn new(triggers: &Vec<triggers::Trigger>) -> Self {
        let logical = filesystem::FsEntry::create_inode();
        let logical_averrage = filesystem::FsEntry::create_inode();
        let logical_averrage_usage = filesystem::FsEntry::create_inode();
        let logical_count = filesystem::FsEntry::create_inode();
        let logical_timestamp = filesystem::FsEntry::create_inode();
        let physical = filesystem::FsEntry::create_inode();
        let physical_count = filesystem::FsEntry::create_inode();
        let physical_timestamp = filesystem::FsEntry::create_inode();

        Self {
            config: config::ModuleConfig::new(),
            system_stats: systemstat::System::new(),
            cpu_stats: None,
            triggers: triggers.to_vec(),
            inode_logical_timestamp: logical_timestamp,
            inode_physical_timestamp: physical_timestamp,
            inode_logical_averrage: logical_averrage,
            inode_logical_averrage_usage: logical_averrage_usage,
            inode_logical_count: logical_count,
            inode_physical_count: physical_count,
            data: CpuListData::new(),
            static_fs_entries: vec![
                filesystem::FsEntry::new(
                    logical,
                    fuse::FileType::Directory,
                    ENTRY_LOGICAL,
                    filesystem::Mode::ReadOnly,
                    &vec![
                        filesystem::FsEntry::new(
                            logical_averrage,
                            fuse::FileType::Directory,
                            ENTRY_AVERRAGE,
                            filesystem::Mode::ReadOnly,
                            &vec![
                                filesystem::FsEntry::new(
                                    logical_averrage_usage,
                                    fuse::FileType::RegularFile,
                                    ENTRY_USAGE,
                                    filesystem::Mode::ReadOnly,
                                    &Vec::new()),
                            ]),

                        filesystem::FsEntry::new(
                            logical_count,
                            fuse::FileType::RegularFile,
                            ENTRY_COUNT,
                            filesystem::Mode::ReadOnly,
                            &Vec::new()),

                        filesystem::FsEntry::new(
                            logical_timestamp,
                            fuse::FileType::RegularFile,
                            ENTRY_TIMESTAMP,
                            filesystem::Mode::ReadOnly,
                            &Vec::new())
                    ]),

                filesystem::FsEntry::new(
                    physical,
                    fuse::FileType::Directory,
                    ENTRY_PHYSICAL,
                    filesystem::Mode::ReadOnly,
                    &vec![
                        filesystem::FsEntry::new(
                            physical_count,
                            fuse::FileType::RegularFile,
                            ENTRY_COUNT,
                            filesystem::Mode::ReadOnly,
                            &Vec::new()),

                        filesystem::FsEntry::new(
                            physical_timestamp,
                            fuse::FileType::RegularFile,
                            ENTRY_TIMESTAMP,
                            filesystem::Mode::ReadOnly,
                            &Vec::new())
                    ]),
                ],
            logical_fs_entries: Vec::new(),
            physical_fs_entries: Vec::new(),
        }
    }

    /// Start system stats monitoring
    fn start_monitoring(&mut self) -> error::Return {
        self.cpu_stats = match self.system_stats.cpu_load() {
            Ok(cpu)=> Some(cpu),
            Err(_) => return error!("Cannot get CPU load"),
        };

        return success!();
    }

    /// Update physical CPU data and filesystem
    fn update_physical(&mut self)
        -> Result<module::Status, error::CerebroError> {

        log::info!("Update physical CPU data");

        let mut status = module::Status::Ok;
        let mut core_temperatures: Vec<u8> = Vec::new();

        let temperature_config = match &self.config.temperature {
            Some(c) => c,
            None => return error!("Missing temperature configuration"),
        };

        let device = match &temperature_config.device {
            Some(d) => d,
            None => return error!("Missing device configuration"),
        };

        let pattern = match &temperature_config.pattern {
            Some(p) => p,
            None => return error!("Missing pattern configuration"),
        };

        let re_pattern = match Regex::new(pattern) {
            Ok(r) => r,
            Err(_) => return error!("Cannot build regex"),
        };

        // Get CPU temperatures
        for chip in Sensors::new() {
            if chip.prefix() != device {
                continue;
            }

            // Search for a temperature feature
            for feature in chip {
                match feature.feature_type() {
                    FeatureType::SENSORS_FEATURE_TEMP => (),
                    _ => continue,
                }

                if ! re_pattern.is_match(feature.name()) {
                    continue;
                }

                // Search for a temperature subfeature
                for subfeature in feature {
                    match subfeature.subfeature_type() {
                        SubfeatureType::SENSORS_SUBFEATURE_TEMP_INPUT => (),
                        _ => continue,
                    }

                    let value = match subfeature.get_value() {
                        Ok(v) => v as u8,
                        Err(_) => continue,
                    };

                    if value == 0 {
                        // Not a valid temperature
                        continue;
                    }

                    core_temperatures.push(value);
                    break;
                }
            }
        }

        // Update CPU count if needed
        let cpu_count = core_temperatures.len();

        if self.data.physical_list.len() != cpu_count {
            status = module::Status::Changed(MODULE_NAME.to_string());

            let old_value = self.data.physical_count.clone();

            self.data.physical_count = format!("{}", cpu_count);

            triggers::find_all_and_execute(
                &self.triggers,
                triggers::Kind::Update,
                MODULE_NAME,
                &format!("{}/{}", ENTRY_PHYSICAL, ENTRY_COUNT),
                &old_value,
                &self.data.physical_count);
        }

        // Rebuild CPU list
        self.data.physical_list.clear();

        for c in core_temperatures {
            self.data.physical_list.push(PhysicalData::new(c as i16));
        }

        // Rebuild filesystem entries if needed
        match status {
            module::Status::Changed(ref _name) => {
                self.physical_fs_entries.clear();

                for i in 0..cpu_count {
                    self.physical_fs_entries.push(
                        filesystem::FsEntry::new(
                            filesystem::FsEntry::create_inode(),
                            fuse::FileType::Directory,
                            &format!("{}", i),
                            filesystem::Mode::ReadOnly,
                            &vec![
                                filesystem::FsEntry::new(
                                    filesystem::FsEntry::create_inode(),
                                    fuse::FileType::RegularFile,
                                    ENTRY_TEMPERATURE,
                                    filesystem::Mode::ReadOnly,
                                    &Vec::new()),
                            ]));
                }
            },

            _ => (),
        }

        self.update_physical_timestamp()?;

        return Ok(status);
    }

    /// Update physical timestamp
    fn update_physical_timestamp(&mut self) -> error::Return {

        let old_value = self.data.physical_timestamp.clone();

        match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => self.data.physical_timestamp = format!("{}", d.as_secs()),
            Err(_) => return error!("Cannot get time since UNIX_EPOCH"),
        }

        // Call triggers if needed
        triggers::find_all_and_execute(
            &self.triggers,
            triggers::Kind::Update,
            MODULE_NAME,
            &format!("{}/{}", ENTRY_PHYSICAL, ENTRY_TIMESTAMP),
            &old_value,
            &self.data.physical_timestamp);

        return success!();
    }

    /// Update logical CPU data and filesystem
    fn update_logical(&mut self)
        -> Result<module::Status, error::CerebroError> {

        log::info!("Update logical CPU data");

        // Get stats
        let stats = match &self.cpu_stats {
            Some(s) => s,
            None => return match self.start_monitoring() {
                Ok(_) => Ok(module::Status::Ok),
                Err(e) => Err(e),
            },
        };

        // Stop monitoring
        let cpu = match stats.done() {
            Ok(c) => c,
            Err(_) => return error!("Cannot read CPU load"),
        };

        // Update CPU averrage if needed
        self.update_logical_cpu_averrage(&cpu)?;

        // Update CPU count if needed
        let status = self.update_logical_cpu_count(&cpu)?;

        match status {
            module::Status::Changed(_) => {
                self.rebuild_logical_filesystem(cpu.len())?;
                self.rebuild_logical_data(&cpu)?;
            },

            _ => self.update_logical_data(&cpu)?,
        }

        self.update_logical_timestamp()?;

        // Restart a monitoring
        self.start_monitoring()?;

        return Ok(status);
    }

    /// Update logical timestamp
    fn update_logical_timestamp(&mut self) -> error::Return {

        let old_value = self.data.logical_timestamp.clone();

        match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => self.data.logical_timestamp = format!("{}", d.as_secs()),
            Err(_) => return error!("Cannot get time since UNIX_EPOCH"),
        }

        // Call triggers if needed
        triggers::find_all_and_execute(
            &self.triggers,
            triggers::Kind::Update,
            MODULE_NAME,
            &format!("{}/{}", ENTRY_LOGICAL, ENTRY_TIMESTAMP),
            &old_value,
            &self.data.logical_timestamp);

        return success!();
    }

    /// Update logical CPU averrage
    fn update_logical_cpu_averrage(&mut self, cpu_list: &Vec<CPULoad>)
        -> error::Return {

        let mut sum: f32 = 0.0;

        let cpu_count = cpu_list.len();

        for c in cpu_list.iter() {
            sum += c.user * 100f32;
        }

        let averrage = format!("{}", sum / (cpu_count as f32));

        if self.data.logical_averrage_usage == averrage {
            return success!();
        }

        // Update data
        let old_value = self.data.logical_averrage_usage.clone();

        self.data.logical_averrage_usage = format!("{}", averrage);

        log::debug!("CPU usage averrage: {}", averrage);

        // Call triggers if needed
        triggers::find_all_and_execute(
            &self.triggers,
            triggers::Kind::Update,
            MODULE_NAME,
            &format!("{}/{}/{}", ENTRY_LOGICAL, ENTRY_AVERRAGE, ENTRY_USAGE),
            &old_value,
            &self.data.logical_averrage_usage);

        return success!();
    }

    /// Update logical CPU count
    fn update_logical_cpu_count(&mut self, cpu_list: &Vec<CPULoad>)
        -> Result<module::Status, error::CerebroError> {

        let cpu_count = cpu_list.len();

        if self.data.logical_list.len() == cpu_count {
            return Ok(module::Status::Ok);
        }

        // Update data
        let old_value = self.data.logical_count.clone();

        self.data.logical_count = format!("{}", cpu_count);

        log::debug!("Number of CPU: {}", cpu_count);

        // Call triggers if needed
        triggers::find_all_and_execute(
            &self.triggers,
            triggers::Kind::Update,
            MODULE_NAME,
            &format!("{}/{}", ENTRY_LOGICAL, ENTRY_COUNT),
            &old_value,
            &self.data.logical_count);

        return Ok(module::Status::Changed(MODULE_NAME.to_string()));
    }

    /// Rebuild logical CPU data
    fn rebuild_logical_data(&mut self, cpu_list: &Vec<CPULoad>)
        -> error::Return {

        // Call delete triggers
        for (index, _data) in self.data.logical_list.iter().enumerate() {
            triggers::find_all_and_execute(
                &self.triggers,
                triggers::Kind::Delete,
                MODULE_NAME,
                &format!("{}/{}/{}", ENTRY_LOGICAL, index, ENTRY_USAGE),
                "",
                "");
        }

        // Rebuild list
        self.data.logical_list.clear();

        for c in cpu_list.iter() {
            self.data.logical_list.push(LogicalData::new(c.user));
        }

        // Call create triggers
        for (index, _data) in self.data.logical_list.iter().enumerate() {
            triggers::find_all_and_execute(
                &self.triggers,
                triggers::Kind::Create,
                MODULE_NAME,
                &format!("{}/{}/{}", ENTRY_LOGICAL, index, ENTRY_USAGE),
                "",
                "");
        }

        return success!();
    }

    /// Update logical CPU data
    fn update_logical_data(&mut self, cpu_list: &Vec<CPULoad>)
        -> error::Return {

        if cpu_list.len() != self.data.logical_list.len() {
            return error!("Cannot update data with a different size");
        }

        for (index, cpu) in cpu_list.iter().enumerate() {
            let data = LogicalData::new(cpu.user);

            if self.data.logical_list[index] == data {
                continue;
            }

            let old_value = self.data.logical_list[index].usage_percent.clone();

            self.data.logical_list[index] = data;

            // Call update trigger
            triggers::find_all_and_execute(
                &self.triggers,
                triggers::Kind::Update,
                MODULE_NAME,
                &format!("{}/{}/{}", ENTRY_LOGICAL, index, ENTRY_USAGE),
                &old_value,
                &self.data.logical_list[index].usage_percent);
        }

        return success!();
    }

    /// Rebuild logical CPU filesystem
    fn rebuild_logical_filesystem(&mut self, cpu_count: usize)
        -> error::Return {

        self.logical_fs_entries.clear();

        for i in 0..cpu_count {
            self.logical_fs_entries.push(
                filesystem::FsEntry::new(
                    filesystem::FsEntry::create_inode(),
                    fuse::FileType::Directory,
                    &format!("{}", i),
                    filesystem::Mode::ReadOnly,
                    &vec![
                        filesystem::FsEntry::new(
                            filesystem::FsEntry::create_inode(),
                            fuse::FileType::RegularFile,
                            ENTRY_USAGE,
                            filesystem::Mode::ReadOnly,
                            &Vec::new()),
                    ]));
        }

        return success!();
    }
}

impl module::Data for CpuBackend {
    /// Update cpu data
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn update(&mut self) -> Result<module::Status, error::CerebroError> {
        let mut status = module::Status::Ok;

        // Logical
        let status_logical = self.update_logical()?;

        match status_logical {
            module::Status::Changed(_) => {
                status = module::Status::Changed(MODULE_NAME.to_string())
            },

            _ => (),
        }

        // Physical
        let status_physical = self.update_physical()?;

        match status_physical {
            module::Status::Changed(_) => {
                status = module::Status::Changed(MODULE_NAME.to_string())
            },

            _ => (),
        }

        return Ok(status);
    }
}

/// Cpu module structure
pub struct Cpu {
    thread: Arc<Mutex<module::Thread>>,
    backend: Arc<Mutex<CpuBackend>>,
}

impl Cpu {
    /// Cpu constructor
    pub fn new(
        event_manager: &mut event_manager::EventManager,
        triggers: &Vec<triggers::Trigger>) -> Self {

        Self {
            thread: Arc::new(Mutex::new(
                module::Thread::new(event_manager.sender()))),

            backend: Arc::new(Mutex::new(CpuBackend::new(triggers))),
        }
    }
}

impl module::Module for Cpu {
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
        let mut backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => return error!("Cannot lock backend"),
        };

        backend.config = config.clone();

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
        return match self.backend.lock() {
            Ok(b) => {
                let mut entries = b.static_fs_entries.to_vec();
                entries[0].fs_entries.extend(b.logical_fs_entries.to_vec());
                entries[1].fs_entries.extend(b.physical_fs_entries.to_vec());
                return entries;
            },

            Err(_) => Vec::new(),
        }
    }

    /// Get value to be displayed for a filesystem entry
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `inode` - The inode of the filesystem to be fetched
    fn value(&self, inode: u64) -> String {
        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => return VALUE_UNKNOWN.to_string(),
        };

        if inode == backend.inode_logical_timestamp {
            return backend.data.logical_timestamp.clone();
        }

        if inode == backend.inode_logical_count {
            return backend.data.logical_count.clone();
        }

        if inode == backend.inode_physical_timestamp {
            return backend.data.physical_timestamp.clone();
        }

        if inode == backend.inode_physical_count {
            return backend.data.physical_count.clone();
        }

        // Search index of entry in logical entries
        for (index, entry) in backend.logical_fs_entries.iter().enumerate() {
            let entry = match entry.find(inode) {
                Some(e) => e,
                None => continue,
            };

            // Entry found, check if index exists
            if index >= backend.data.logical_list.len() {
                return VALUE_UNKNOWN.to_string();
            }

            // Get data
            let cpu_data = &backend.data.logical_list[index];

            match entry.name.as_str() {
                ENTRY_USAGE => return cpu_data.usage_percent.to_string(),
                _ => return VALUE_UNKNOWN.to_string(),
            }
        }

        // Search index of entry in physical entries
        for (index, entry) in backend.physical_fs_entries.iter().enumerate() {
            let entry = match entry.find(inode) {
                Some(e) => e,
                None => continue,
            };

            // Entry found, check if index exists
            if index >= backend.data.physical_list.len() {
                return VALUE_UNKNOWN.to_string();
            }

            // Get data
            let cpu_data = &backend.data.physical_list[index];

            match entry.name.as_str() {
                ENTRY_TEMPERATURE => return cpu_data.temperature.to_string(),
                _ => return VALUE_UNKNOWN.to_string(),
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

        let mut output: String = format!(
            "logical_cpu_count={} logical_averrage_usage={}",
            backend.data.logical_count,
            backend.data.logical_averrage_usage);

        output +=
            &format!(" physical_cpu_count={}", backend.data.physical_count);

        for (index, cpu) in backend.data.logical_list.iter().enumerate() {
            output += &format!(
                " logical_cpu_{}_usage={}",
                index,
                cpu.usage_percent);
        }

        for (index, cpu) in backend.data.physical_list.iter().enumerate() {
            output += &format!(
                " physical_cpu_{}_temperature={}",
                index,
                cpu.temperature);
        }

        return output;
    }
}
