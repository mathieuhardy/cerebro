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

const MODULE_NAME: &str = "brightness";

const VALUE_UNKNOWN: &str = "?";

const ENTRY_VALUE: &str = "value";
const ENTRY_CURRENT_VALUE: &str = "current_value";

/// Information about the brightness
#[derive(Serialize)]
struct BrightnessData
{
    pub device: String,
    pub value: String,
    pub current_value: String,
}

/// Proxy backend that is only use in the context of the thread
struct BrightnessBackendProxy {
    backend: Arc<Mutex<BrightnessBackend>>,
}

impl BrightnessBackendProxy {
    fn new(backend: Arc<Mutex<BrightnessBackend>>) -> Self {
        Self {
            backend: backend,
        }
    }
}

impl module::Data for BrightnessBackendProxy {
    /// Update trash data
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    fn update(&mut self) -> Result<module::Status, error::CerebroError> {
        // Check if the fileystem needs to be built
        let status = match self.backend.lock() {
            Ok(mut b) => b.build_filesystem()?,
            Err(_) => return error!("Cannot lock backend"),
        };

        match status {
            module::Status::Changed(_) => return Ok(status),
            _ => (),
        }

        // Get entries
        let root = path::Path::new("/")
            .join("sys")
            .join("class")
            .join("backlight");

        let devices = fs::read_dir(&root).unwrap();

        // Create watcher
        let (tx, rx) = mpsc::channel();

        let mut w: notify::INotifyWatcher = match notify::Watcher::new_raw(tx) {
            Ok(w) => w,
            Err(_) => return error!("Cannot create filesystem watcher"),
        };

        // Watch each device
        for device in devices {
            let device = match device {
                Ok(d) => d,
                Err(_) => continue,
            };

            let path = root.join(device.file_name()).join("brightness");

            if ! path.exists() {
                continue;
            }

            match w.watch(&path, notify::RecursiveMode::NonRecursive) {
                Ok(_) => (),
                Err(_) => return error!("Cannot add path to watch"),
            }

            println!("+++ watch {:?}", path);
        }

        loop {
            let event = match rx.recv() {
                Ok(e) => e,
                Err(_) => return error!("Error during watching filesystem"),
            };

            println!("!!!! {:#?}", event);

            //let op = match event.op {
                //Ok(o) => o,
                //Err(_) => return error!("Watch event returned an error"),
            //};

            //match op {
                //notify::Op::CREATE | notify::Op::CLOSE_WRITE => (),
                //_ => continue,
            //}

            //self.update_count()?;
        }
    }
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

    fn build_filesystem(&mut self)
        -> Result<module::Status, error::CerebroError> {

        if ! self.fs_entries.is_empty() {
            return Ok(module::Status::Ok);
        }

        let root = path::Path::new("/")
            .join("sys")
            .join("class")
            .join("backlight");

        let devices = fs::read_dir(&root).unwrap();

        // Build data
        self.data.clear();

        for device in devices {
            let name = match device {
                Ok(d) => d.file_name(),
                Err(_) => continue,
            };

            let name = match name.into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };

            let value_path = root.join(&name).join("brightness");
            let value = match fs::read_to_string(&value_path) {
                Ok(v) => v.replace("\n", ""),
                Err(_) => {
                    println!("Cannot read content of: {:?}", value_path);
                    continue;
                },
            };

            let current_value_path = root.join(&name).join("actual_brightness");
            let current_value = match fs::read_to_string(&current_value_path) {
                Ok(v) => v.replace("\n", ""),
                Err(_) => {
                    println!("Cannot read content of: {:?}", value_path);
                    continue;
                },
            };

            self.data.push(BrightnessData{
                device: name,
                value: value,
                current_value: current_value,
            });
        }

        // Build filesystem
        for data in self.data.iter() {
            self.fs_entries.push(filesystem::FsEntry::new(
                filesystem::FsEntry::create_inode(),
                fuse::FileType::Directory,
                &data.device,
                filesystem::Mode::ReadOnly,
                &vec![
                    filesystem::FsEntry::new(
                        filesystem::FsEntry::create_inode(),
                        fuse::FileType::RegularFile,
                        ENTRY_VALUE,
                        filesystem::Mode::ReadOnly, //TODO: read-write
                        &Vec::new()),

                    filesystem::FsEntry::new(
                        filesystem::FsEntry::create_inode(),
                        fuse::FileType::RegularFile,
                        ENTRY_CURRENT_VALUE,
                        filesystem::Mode::ReadOnly, //TODO: read-write
                        &Vec::new()),
                ]));

            // Creation triggers
            triggers::find_all_and_execute(
                &self.triggers,
                triggers::Kind::Create,
                MODULE_NAME,
                &data.device);
        }

        return Ok(module::Status::Changed(MODULE_NAME.to_string()));
    }
}

/// Brightness module structure
pub struct Brightness {
    thread: Arc<Mutex<module::Thread>>,
    //inode_count: u64,
    //inode_empty: u64,
    backend: Arc<Mutex<BrightnessBackend>>,
    backend_proxy: Arc<Mutex<BrightnessBackendProxy>>,
}

impl Brightness {
    /// Brightness constructor
    pub fn new(
        event_manager: &mut event_manager::EventManager,
        triggers: &Vec<triggers::Trigger>) -> Self {

        //let count = filesystem::FsEntry::create_inode();
        //let empty = filesystem::FsEntry::create_inode();
        let backend = Arc::new(Mutex::new(BrightnessBackend::new(triggers)));

        Self {
            thread: Arc::new(Mutex::new(
                module::Thread::new(event_manager.sender()))),

            //inode_count: count,
            //inode_empty: empty,
            backend: backend.clone(),
            backend_proxy:
                Arc::new(
                    Mutex::new(
                        BrightnessBackendProxy::new(backend.clone()))),
            //fs_entries: vec![
                //filesystem::FsEntry::new(
                    //count,
                    //fuse::FileType::RegularFile,
                    //ENTRY_COUNT,
                    //filesystem::Mode::ReadOnly,
                    //&Vec::new()),

                //filesystem::FsEntry::new(
                    //empty,
                    //fuse::FileType::RegularFile,
                    //ENTRY_EMPTY,
                    //filesystem::Mode::WriteOnly,
                    //&Vec::new())
                //],
        }
    }
}

impl module::Module for Brightness {
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
        // Find filesystem entry
        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => return VALUE_UNKNOWN.to_string(),
        };

        for device_entry in backend.fs_entries.iter() {
            let entry = match device_entry.fs_entries
                .iter().find(|x| x.inode == inode) {

                Some(e) => e,
                None => continue,
            };

            // Find corresponding data
            let data =
                match backend.data
                .iter().find(|x| x.device == device_entry.name) {

                Some(d) => d,
                None => return VALUE_UNKNOWN.to_string(),
            };

            return match entry.name.as_str() {
                ENTRY_VALUE => data.value.clone(),
                ENTRY_CURRENT_VALUE => data.current_value.clone(),
                _ => VALUE_UNKNOWN.to_string(),
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
    fn set_value(&mut self, inode: u64, data: &[u8]) {
        //if inode == self.inode_empty {
            //match data {
                //b"1" | b"1\n" | b"true" | b"true\n" => {
                    //let _backend = match self.backend.lock() {
                        //Ok(b) => b,
                        //Err(_) => {
                            //println!("Cannot lock backend");
                            //return;
                        //},
                    //};

                    //let home_dir = match dirs::home_dir() {
                        //Some(path) => path,
                        //None => {
                            //println!("Cannot get home directory");
                            //return;
                        //},
                    //};

                    //let trash_dir = home_dir
                        //.join(".local")
                        //.join("share")
                        //.join("Trash");

                    //let dir = trash_dir.join("files");

                    //match Trash::remove_dir_contents(&dir) {
                        //Ok(_) => (),
                        //Err(_) => println!("Cannot empty directory: {:?}", dir),
                    //}

                    //let dir = trash_dir.join("info");

                    //match Trash::remove_dir_contents(&dir) {
                        //Ok(_) => (),
                        //Err(_) => println!("Cannot empty directory: {:?}", dir),
                    //}
                //},

                //_ => (),
            //}
        //}
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
        return VALUE_UNKNOWN.to_string();
        //let backend = match self.backend.lock() {
            //Ok(b) => b,
            //Err(_) => return VALUE_UNKNOWN.to_string(),
        //};

        //return format!("count={}", backend.data.count).to_string();
    }
}
