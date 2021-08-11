use async_mutex;
use brightness::Brightness;
use fuse;
use futures::{executor, TryStreamExt};
use serde::{Serialize};
use std::sync::{Arc, Mutex};

use crate::config;
use crate::error;
use crate::event_manager;
use crate::filesystem;
use crate::modules::module;
use crate::triggers;


const MODULE_NAME: &str = "brightness";

const VALUE_UNKNOWN: &str = "?";

//const ENTRY_PLUGGED: &str = "plugged";

/// Information about the brightness
#[derive(Clone, Debug, Serialize)]
struct BrightnessData
{
    pub device: String,
    pub value: String,
}

/// Brightness backend that will compute the values
struct BrightnessBackend {
    triggers: Vec<triggers::Trigger>,

    pub data: Vec<BrightnessData>,
    pub fs_entries: Vec<filesystem::FsEntry>,
}

impl BrightnessBackend {
    fn new(triggers: &Vec<triggers::Trigger>) -> Self {
        Self {
            triggers: triggers.to_vec(),
            data: Vec::new(),
            fs_entries: Vec::new(),
        }
    }

    fn rebuild_fs_entries(&mut self) -> error::CerebroResult {
        self.fs_entries.clear();

        for d in self.data.iter() {
            self.fs_entries.push(
                filesystem::FsEntry::new(
                    filesystem::FsEntry::create_inode(),
                    fuse::FileType::RegularFile,
                    &d.device,
                    filesystem::Mode::ReadOnly,
                    &Vec::new()));
        }

        return Success!();
    }

    fn update_brightness_values(&mut self, data: &Vec<BrightnessData>)
        -> Result<module::Status, error::CerebroError> {

        let mut changed: bool = false;

        if self.data.len() != data.len() {
            changed = true;
        }
        else {
            for d in data.iter() {
                match self.data.iter().find(|&x| x.device == d.device) {
                    Some(_) => (),
                    None => {
                        changed = true;
                        break;
                    },
                }
            }
        }

        // If changed: clear the list and assign the new one
        if changed {
            // Call delete triggers
            for data in self.data.iter() {
                triggers::find_all_and_execute(
                    &self.triggers,
                    triggers::Kind::Delete,
                    MODULE_NAME,
                    &format!("{}", data.device));
            }

            // Rebuild list
            self.data = data.to_vec();

            // Call create triggers
            for data in self.data.iter() {
                triggers::find_all_and_execute(
                    &self.triggers,
                    triggers::Kind::Create,
                    MODULE_NAME,
                    &format!("{}", data.device));
            }

            // Rebuild filesystem entries
            self.rebuild_fs_entries()?;

            return Ok(module::Status::Changed(MODULE_NAME.to_string()));
        }

        // Simply update values
        for d in data.iter() {
            match self.data.iter_mut().find(|x| x.device == d.device) {
                Some(entry) => {
                    entry.value = d.value.clone();

                    triggers::find_all_and_execute(
                        &self.triggers,
                        triggers::Kind::Update,
                        MODULE_NAME,
                        &format!("{}", entry.device));
                },

                None => return error!("Device entry must be found"),
            }
        }

        return Ok(module::Status::Ok);
    }

    async fn fetch_brightness_values(
        &mut self,
        data: &Arc<async_mutex::Mutex<Vec<BrightnessData>>>)
        -> error::CerebroResult {

        let stream = brightness::brightness_devices();

        let task = stream.try_for_each(|dev| async move {
            let mut data = data.lock().await;

            data.push(BrightnessData{
                device: dev.device_name().await?,
                value: format!("{}", dev.get().await?),
            });

            return Ok(());
        });

        return match task.await {
            Ok(_) => Success!(),
            Err(_) => error!("Error during fetching of brightness"),
        }
    }
}

impl module::Data for BrightnessBackend {
    /// Update brightness data
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn update(&mut self) -> Result<module::Status, error::CerebroError> {
        let data = Arc::new(async_mutex::Mutex::new(Vec::new()));

        executor::block_on(self.fetch_brightness_values(&data))?;

        let status = executor::block_on(async {
            println!("!!!!!!!!!!!!!!!! {:?}", data.lock().await);
            let data = &data.lock().await.to_vec();

            return self.update_brightness_values(data);
        })?;

        return Ok(status);
    }
}

/// Brightness module structure
pub struct BrightnessModule {
    thread: Arc<Mutex<module::Thread>>,
    //inode_plugged: u64,
    backend: Arc<Mutex<BrightnessBackend>>,
}

impl BrightnessModule {
    /// Brightness constructor
    pub fn new(
        event_manager: &mut event_manager::EventManager,
        triggers: &Vec<triggers::Trigger>) -> Self {

        //let plugged = filesystem::FsEntry::create_inode();

        Self {
            thread: Arc::new(Mutex::new(
                module::Thread::new(event_manager.sender()))),

            //inode_plugged: plugged,
            backend: Arc::new(Mutex::new(BrightnessBackend::new(triggers))),
        }
    }
}

impl module::Module for BrightnessModule {
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
        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };

        return backend.fs_entries.to_vec();
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

        // Search device that matched the inode
        let device_name =
            match backend.fs_entries.iter().find(|x| x.inode == inode) {
                Some(e) => e.name.clone(),
                None => return VALUE_UNKNOWN.to_string(),
            };

        return match backend.data.iter().find(|x| x.device == device_name) {
            Some(e) => e.value.clone(),
            None => VALUE_UNKNOWN.to_string(),
        }
    }

    /// Set value of a filesystem entry
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `inode` - The inode of the filesystem to be written
    /// * `data` - The data to be written
    fn set_value(&mut self, _inode: u64, _data: &[u8]) {
        //TODO
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

        let mut output = "".to_string();

        for (index, data) in backend.data.iter().enumerate() {
            output += &format!("device_{}_name={} device_{}_brightness={}",
                index,
                data.device,
                index,
                data.value);
        }

        return output;
    }
}
