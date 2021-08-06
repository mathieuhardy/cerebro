use lazy_static::lazy_static;
use libc::ENOENT;
use std::cmp;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::thread;

use fuse::{
    FileAttr,
    Filesystem,
    FileType,
    ReplyAttr,
    ReplyData,
    ReplyDirectory,
    ReplyEntry,
    ReplyWrite,
    Request};

use crate::config;
use crate::event_manager;
use crate::events;
use crate::modules::module;

const INODE_INVALID: u64 = 0;
const INODE_ROOT: u64 = 1;

const ENTRY_JSON: &str = "json";
const ENTRY_SHELL: &str = "shell";

const TTL: Duration = Duration::from_secs(1);

lazy_static! {
    static ref INODE_INDEX: Mutex<u64> = Mutex::new(INODE_ROOT);
}

/// Filesystem entry: file or directory
#[derive(Debug, Clone)]
pub struct FsEntry {
    pub inode: u64,
    pub file_type: FileType,
    pub name: String,
    pub write_only: bool,
    pub fs_entries: Vec<FsEntry>,
}

impl FsEntry {
    /// FsEntry constructor
    pub fn new(
        inode: u64,
        file_type: FileType,
        name: &str,
        write_only: bool,
        fs_entries: &Vec<FsEntry>) -> Self {

        Self {
            inode: inode,
            file_type: file_type,
            name: name.to_string(),
            write_only: write_only,
            fs_entries: fs_entries.to_vec(),
        }
    }

    /// Create a new unique inode value
    pub fn create_inode() -> u64 {
        let mut guard = match INODE_INDEX.lock() {
            Ok(g) => g,
            Err(_) => {
                log::error!("Cannot lock inode index");
                return INODE_INVALID;
            },
        };

        *guard = *guard + 1;
        return *guard;
    }

    /// Get attributes of the filesystem entry
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `size` - The size in bytes of the content of the entry
    pub fn attrs(&self, size: u32) -> FileAttr {
        let perm = match self.file_type {
            FileType::RegularFile => match self.write_only {
                true => 0o222,
                false => 0o444,
            },
            _ => 0o555,
        };

        let blocks = match self.file_type {
            FileType::RegularFile => 1,
            _ => 0,
        };

        let nlink = match self.file_type {
            FileType::RegularFile => 1,
            _ => 2,
        };

        FileAttr {
            ino: self.inode,
            size: size as u64,
            blocks: blocks,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: self.file_type,
            perm: perm,
            nlink: nlink,
            uid: 0,
            gid: 0,
            rdev: 0,
            flags: 0,
        }
    }

    /// Find a filesystem entry into the current one
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `inode` - The inode of the entry to search
    pub fn find<'i>(&'i self, inode: u64) -> Option<&'i FsEntry> {
        if self.inode == inode {
            return Some(self);
        }

        for entry in self.fs_entries.iter() {
            match entry.find(inode) {
                Some(e) => return Some(e),
                None => (),
            }
        }

        return None;
    }

    /// Find a filesystem entry into the current one by its name
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `name` - The name of the entry to search
    pub fn find_by_name<'i>(&'i self, name: &str) -> Option<&'i FsEntry> {
        if self.name == name {
            return Some(self);
        }

        for entry in self.fs_entries.iter() {
            match entry.find_by_name(name) {
                Some(e) => return Some(e),
                None => (),
            }
        }

        return None;
    }
}

/// Filesystem backend structure used to store data
pub struct FsBackend {
    root: FsEntry,
    modules: Vec<Arc<Mutex<dyn module::Module>>>,
    config: config::Config,
}

impl FsBackend {
    /// Constructor
    pub fn new(
        modules: &Vec<Arc<Mutex<dyn module::Module>>>,
        config: &config::Config) -> Self {

        Self {
            root: FsEntry::new(
                INODE_ROOT,
                FileType::Directory,
                "/",
                false,
                &Vec::new()),
            modules: modules.to_vec(),
            config: config.clone(),
        }
    }

    /// Find the module by its name
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `name` - The name of the module to find
    pub fn find_module_by_name(&self, name: String)
        -> Option<Arc<Mutex<dyn module::Module>>> {

        for m in self.modules.iter() {
            let module = match m.lock() {
                Ok(m) => m,
                Err(_) => continue,
            };

            if module.name() == name {
                return Some(m.clone());
            }
        }

        return None;
    }

    /// Find the module that owns a filesystem entry
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `inode` - The inode of the entry to search
    pub fn find_module(&self, inode: u64)
        -> Option<&Arc<Mutex<dyn module::Module>>> {

        // First search with the inode
        for m in self.modules.iter() {
            let module = match m.lock() {
                Ok(m) => m,
                Err(_) => continue,
            };

            for entry in module.fs_entries().iter() {
                match entry.find(inode) {
                    Some(_) => return Some(m),
                    None => (),
                }
            }
        }

        return None;
    }

    /// Register a module in to the filesystem giving its name
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `name` - The name of the module to register
    pub fn register_module_by_name(&mut self, name: String) {
        match self.find_module_by_name(name) {
            Some(m) => {
                FsBackend::register_module(&self.config, m, &mut self.root);
            },

            None => (),
        }
    }

    /// Register a module in to the filesystem
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    pub fn register_module(
        config: &config::Config,
        module: Arc<Mutex<dyn module::Module>>,
        root: &mut FsEntry) {

        let mut module = match module.lock() {
            Ok(m) => m,
            Err(_) => return,
        };

        if ! config.modules.contains_key(module.name()) {
            // No JSON config: consider that it's not enabled
            return;
        }

        let config = &config.modules[module.name()];

        // Check if enabled
        match config.enabled {
            Some(true) => (),
            _ => return,
        }

        // Stop module
        log::info!("stop module: {}", module.name());

        match module.stop() {
            Ok(_) => (),
            Err(e) => {
                log::error!("Cannot stop module: {}", e);
                return;
            },
        }

        // Unregister its old filesystem
        let index = match root.fs_entries.iter().position(
            |x| x.name == module.name()) {

            Some(i) => i,
            None => usize::MAX,
        };

        if index != usize::MAX {
            root.fs_entries.remove(index);
        }

        // Register its filesystem
        match root.fs_entries.iter().find(|x| &x.name == module.name()) {
            Some(_) => log::debug!("Module is already registered"),
            None => (),
        }

        let mut entry = FsEntry::new(
            FsEntry::create_inode(),
            FileType::Directory,
            module.name(),
            false,
            &module.fs_entries());

        FsBackend::register_custom_entries(config, &mut entry);

        root.fs_entries.push(entry);

        // Start module
        log::info!("start module: {}", module.name());

        match module.start(&config) {
            Ok(_) => (),
            Err(e) => log::error!("Cannot start module: {}", e),
        }
    }

    /// Register modules into the filesystem
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    pub fn register_modules(&mut self) {
        self.root.fs_entries.clear();

        for m in self.modules.iter_mut() {
            FsBackend::register_module(&self.config, m.clone(), &mut self.root);
        }
    }

    /// Add custom filesystem entries to a module filesystem tree
    ///
    /// # Arguments
    ///
    /// * `self` - The instance handle
    /// * `config` - Module configuration
    /// * `entry` - Filesystem entry of the module
    fn register_custom_entries(
        config: &config::ModuleConfig,
        entry: &mut FsEntry) {

        // JSON
        match &config.json {
            Some(c) => {
                match c.enabled {
                    Some(true) => {
                        entry.fs_entries.push(FsEntry::new(
                            FsEntry::create_inode(),
                            FileType::RegularFile,
                            ENTRY_JSON,
                            false,
                            &Vec::new()));
                    },

                    _ => (),
                }
            },

            None => (),
        }

        // Shell
        match &config.shell {
            Some(c) => {
                match c.enabled {
                    Some(true) => {
                        entry.fs_entries.push(FsEntry::new(
                            FsEntry::create_inode(),
                            FileType::RegularFile,
                            ENTRY_SHELL,
                            false,
                            &Vec::new()));
                    },

                    _ => (),
                }
            },

            None => (),
        }
    }
}

/// Filesystem struct implementing fuse methods
pub struct Fs {
    backend: Arc<Mutex<FsBackend>>,
    receiver: Arc<Mutex<Receiver<events::Events>>>,
}

impl Fs {
    /// Constructor
    pub fn new(
        modules: &Vec<Arc<Mutex<dyn module::Module>>>,
        config: &config::Config,
        event_manager: &mut event_manager::EventManager) -> Self {

        Self {
            backend: Arc::new(Mutex::new(FsBackend::new(modules, config))),
            receiver: event_manager.receiver(),
        }
    }
}

impl Filesystem for Fs {
    fn init(&mut self, _req: &Request) -> Result<(), i32> {
        // Start event management thread
        let receiver = self.receiver.clone();
        let backend = self.backend.clone();

        thread::spawn(move || loop {
            let rx = match receiver.lock() {
                Ok(r) => r,
                Err(_) => continue,
            };

            let event = match rx.recv() {
                Ok(event) => event,
                Err(_) => continue,
            };

            let mut backend = match backend.lock() {
                Ok(b) => b,
                Err(_) => continue,
            };

            match event {
                events::Events::ModuleUpdated(module) => {
                    backend.register_module_by_name(module);
                },
            }
        });

        // Register filesystems and start modules
        match self.backend.lock() {
            Ok(mut b) => b.register_modules(),
            Err(_) => (),
        }

        return Ok(());
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory) {

        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => {
                reply.error(ENOENT);
                return;
            },
        };

        let mut entries = vec![
            (INODE_ROOT, FileType::Directory, "."),
            (INODE_ROOT, FileType::Directory, ".."),
        ];

        match backend.root.find(ino) {
            Some(entry) => {
                for e in entry.fs_entries.iter() {
                    entries.push((e.inode, e.file_type, &e.name));
                }
            },

            None => (),
        }

        for (i, entry) in
            entries.into_iter().enumerate().skip(offset as usize) {

            // i + 1 means the index of the next entry
            reply.add(entry.0, (i + 1) as i64, entry.1, entry.2);
        }

        reply.ok();
    }

    fn lookup(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        reply: ReplyEntry) {

        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => {
                reply.error(ENOENT);
                return;
            },
        };

        let entry_name: &str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            },
        };

        // Search parent
        let parent_entry = match backend.root.find(parent) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            },
        };

        // Search entry
        let entry = match parent_entry.find_by_name(&entry_name) {
            Some(e) => e,
            None => {
                reply.error(ENOENT);
                return;
            },
        };

        if entry.file_type == FileType::Directory {
            reply.entry(&TTL, &entry.attrs(0), 0);
            return;
        }

        // Try to find the module owning this entry
        match backend.find_module(entry.inode) {
            Some(m) => {
                match m.lock() {
                    Ok(m) => {
                        let size = m.value(entry.inode).as_bytes().len() as u32;
                        reply.entry(&TTL, &entry.attrs(size), 0);
                        return;
                    },

                    Err(_) => (),
                }
            },

            None => (),
        }

        // It must be a custom entry (json, ...)
        for module in backend.modules.iter() {
            let module = match module.lock() {
                Ok(m) => m,
                Err(_) => continue,
            };

            if module.name() != parent_entry.name {
                continue;
            }

            let size = match entry.name.as_str() {
                ENTRY_JSON => module.json().as_bytes().len() as u32,
                ENTRY_SHELL => module.shell().as_bytes().len() as u32,
                _ => 0,
            };

            reply.entry(&TTL, &entry.attrs(size), 0);

            return;
        }

        reply.error(ENOENT);
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => {
                reply.error(ENOENT);
                return;
            },
        };

        // Find entry
        let entry = match backend.root.find(ino) {
            Some(e) => e,
            None => {
                reply.error(ENOENT);
                return;
            },
        };

        if entry.file_type == FileType::Directory {
            reply.attr(&TTL, &entry.attrs(0));
            return;
        }

        // Try to find the module owning this entry
        match backend.find_module(entry.inode) {
            Some(m) => {
                match m.lock() {
                    Ok(m) => {
                        let size = m.value(entry.inode).as_bytes().len() as u32;
                        reply.attr(&TTL, &entry.attrs(size));
                        return;
                    },

                    Err(_) => (),
                }
            },

            None => (),
        }

        // It must be a custom entry (json, ...)
        for module_entry in backend.root.fs_entries.iter() {
            match module_entry.find(entry.inode) {
                Some(_) => (),
                None => continue,
            }

            for module in backend.modules.iter() {
                let module = match module.lock() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                if module.name() != module_entry.name {
                    continue;
                }

                let size = match entry.name.as_str() {
                    ENTRY_JSON => module.json().as_bytes().len() as u32,
                    ENTRY_SHELL => module.shell().as_bytes().len() as u32,
                    _ => 0,
                };

                reply.attr(&TTL, &entry.attrs(size));

                return;
            }

            break;
        }

        reply.error(ENOENT);
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        reply: ReplyData) {

        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => {
                reply.error(ENOENT);
                return;
            },
        };

        // Find entry
        let entry = match backend.root.find(ino) {
            Some(e) => e,
            None => {
                reply.error(ENOENT);
                return;
            },
        };

        if entry.write_only {
            reply.error(ENOENT);
            return;
        }

        // Try to find the module owning this entry
        match backend.find_module(entry.inode) {
            Some(m) => {
                match m.lock() {
                    Ok(m) => {
                        let value = m.value(entry.inode).to_string();
                        let bytes = value.as_bytes();
                        let length = bytes.len() as u32;

                        if offset >= 0 && (offset as u32) < length {
                            let size = cmp::min(size, length);
                            reply.data(&bytes[offset as usize..size as usize]);
                        }

                        return;
                    },

                    Err(_) => (),
                }
            },

            None => (),
        }

        // It must be a custom entry (json, ...)
        for module_entry in backend.root.fs_entries.iter() {
            match module_entry.find(entry.inode) {
                Some(_) => (),
                None => continue,
            }

            for module in backend.modules.iter() {
                let module = match module.lock() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                if module.name() != module_entry.name {
                    continue;
                }

                let value = match entry.name.as_str() {
                    ENTRY_JSON => module.json().to_string(),
                    ENTRY_SHELL => module.shell().to_string(),
                    _ => {
                        reply.error(ENOENT);
                        return;
                    },
                };

                let bytes = value.as_bytes();
                let length = bytes.len() as u32;

                if offset >= 0 && (offset as u32) < length {
                    let size = cmp::min(size, length);
                    reply.data(&bytes[offset as usize..size as usize]);
                }

                return;
            }

            break;
        }

        reply.error(ENOENT);
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        _offset: i64,
        data: &[u8],
        _flags: u32,
        reply: ReplyWrite) {

        let backend = match self.backend.lock() {
            Ok(b) => b,
            Err(_) => {
                reply.error(ENOENT);
                return;
            },
        };

        // Find entry
        let entry = match backend.root.find(ino) {
            Some(e) => e,
            None => {
                reply.error(ENOENT);
                return;
            },
        };

        if ! entry.write_only {
            reply.error(ENOENT);
            return;
        }

        // Try to find the module owning this entry
        match backend.find_module(entry.inode) {
            Some(m) => {
                match m.lock() {
                    Ok(mut m) => {
                        m.set_value(entry.inode, data);
                        reply.written(data.len() as u32);
                        return;
                    },

                    Err(_) => (),
                }
            },

            None => (),
        }

        reply.error(ENOENT);
    }

    fn setattr(
        &mut self,
        req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<SystemTime>,
        _mtime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr)
    {
        self.getattr(req, ino, reply);
    }
}

/// Frontend filesysem struture
pub struct FsFrontend {
    fs: Arc<Mutex<Fs>>,
}

impl FsFrontend {
    /// Constructor
    pub fn new(fs: &Arc<Mutex<Fs>>) -> Self {
        Self {
            fs: fs.clone(),
        }
    }
}

impl Filesystem for FsFrontend {
    fn init(&mut self, _req: &Request) -> Result<(), i32> {
        let mut fs = match self.fs.lock() {
            Ok(f) => f,
            Err(_) => return Err(-1),
        };

        return fs.init(_req);
    }

    fn readdir(
        &mut self,
        req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        reply: ReplyDirectory) {

        let mut fs = match self.fs.lock() {
            Ok(f) => f,
            Err(_) => return,
        };

        fs.readdir(req, ino, fh, offset, reply);
    }

    fn lookup(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        reply: ReplyEntry) {

        let mut fs = match self.fs.lock() {
            Ok(f) => f,
            Err(_) => return,
        };

        fs.lookup(req, parent, name, reply);
    }

    fn getattr(&mut self, req: &Request, ino: u64, reply: ReplyAttr) {
        let mut fs = match self.fs.lock() {
            Ok(f) => f,
            Err(_) => return,
        };

        fs.getattr(req, ino, reply);
    }

    fn read(
        &mut self,
        req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        reply: ReplyData) {

        let mut fs = match self.fs.lock() {
            Ok(f) => f,
            Err(_) => return,
        };

        fs.read(req, ino, fh, offset, size, reply);
    }

    fn write(
        &mut self,
        req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        flags: u32,
        reply: ReplyWrite) {

        let mut fs = match self.fs.lock() {
            Ok(f) => f,
            Err(_) => return,
        };

        fs.write(req, ino, fh, offset, data, flags, reply);
    }

    fn setattr(
        &mut self,
        req: &Request,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
        fh: Option<u64>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<u32>,
        reply: ReplyAttr)
    {
        let mut fs = match self.fs.lock() {
            Ok(f) => f,
            Err(_) => return,
        };

        fs.setattr(
            req,
            ino,
            mode,
            uid,
            gid,
            size,
            atime,
            mtime,
            fh,
            crtime,
            chgtime,
            bkuptime,
            flags,
            reply);
    }
}
