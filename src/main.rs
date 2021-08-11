#[macro_use]
mod error;

mod config;
mod event_manager;
mod events;
mod filesystem;
mod modules;
mod triggers;

use clap;
use dirs;
use env_logger;
use fuse;
use log4rs::append::file::FileAppender;
use log4rs::config::{Appender, Config, Root};
use std::ffi::OsStr;
use std::fs;
use std::sync::Arc;
use std::sync::Mutex;

use modules::cpu;
use modules::battery;
use modules::brightness;
use modules::Module;
use modules::trash;

fn main() {
    // Command line interface
    let mut mountpoint: String = "/tmp/cerebro".to_string();
    let mut log_file: Option<String> = None;

    let app = clap::App::new("NixOS setup")
        .version("1.0.0")
        .author("Mathieu H. <mhardy2008@gmail.com>")
        .about("Monitor system information")
        .arg(clap::Arg::with_name("mountpoint")
            .short("m")
            .long("mountpoint")
            .help("Path where the filesystem will be mounted")
            .required(false)
            .takes_value(true))
        .arg(clap::Arg::with_name("logfile")
            .short("l")
            .long("logfile")
            .help("Path of a file where the logs should be printed")
            .required(false)
            .takes_value(true));

    let matches = app.get_matches();

    for arg in matches.args.iter() {
        match arg.0 {
            &"mountpoint" => {
                match matches.value_of(arg.0) {
                    Some(s) => mountpoint = s.to_string(),
                    None => (),
                }
            },

            &"logfile" => {
                match matches.value_of(arg.0) {
                    Some(s) => log_file = Some(s.to_string()),
                    None => (),
                }
            },

            _ => (),
        }
    }

    // Configure logs
    match log_file {
        Some(l) => {
            let f = FileAppender::builder().build(l).unwrap();

            let config = Config::builder()
                .appender(Appender::builder().build("logfile", Box::new(f)))
                .build(Root::builder()
                    .appender("logfile")
                    .build(log::LevelFilter::Trace)).unwrap();

            log4rs::init_config(config).unwrap();
        },

        None => {
            env_logger::Builder::new()
                .filter(None, log::LevelFilter::Trace)
                .format_timestamp(None)
                .format_module_path(false)
                .init();
        },
    }

    // Load configuration
    let home_dir = match dirs::home_dir() {
        Some(path) => path,
        None => {
            log::error!("Cannot get home directory");
            return;
        }
    };

    let config_dir = home_dir.join(".config").join("cerebro");
    let config_file = config_dir.join("config.json");

    let config = match config::load(config_file) {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error loading configuration: {}", e);
            return;
        }
    };

    log::info!("{:#?}", config);

    // Load triggers
    let triggers = match triggers::load(config_dir) {
        Ok(t) => t,
        Err(e) => {
            log::error!("Error loading triggers: {}", e);
            return;
        },
    };

    log::info!("{:#?}", triggers);

    // Event manager
    let mut event_manager = event_manager::EventManager::new();

    // List of modules
    let mut modules: Vec<Arc<Mutex<dyn Module>>> = Vec::new();

    modules.push(Arc::new(Mutex::new(cpu::Cpu::new(
        &mut event_manager,
        &triggers))));

    modules.push(Arc::new(Mutex::new(battery::Battery::new(
        &mut event_manager,
        &triggers))));

    modules.push(Arc::new(Mutex::new(brightness::Brightness::new(
        &mut event_manager,
        &triggers))));

    modules.push(Arc::new(Mutex::new(trash::Trash::new(
        &mut event_manager,
        &triggers))));

    // Create filesystem
    let fs = Arc::new(Mutex::new(filesystem::Fs::new(
        &modules,
        &config,
        &mut event_manager)));

    let fs_frontend = filesystem::FsFrontend::new(&fs);

    log::info!("Mountpoint is: {}", &mountpoint);

    match fs::create_dir_all(&mountpoint) {
        Ok(_) => (),
        Err(_) => {
            log::error!("Cannot create mountpoint");
            return;
        },
    }

    let options = ["-o", "fsname=cerebro"]
        .iter()
        .map(|o| o.as_ref())
        .collect::<Vec<&OsStr>>();

    match fuse::mount(fs_frontend, mountpoint, &options) {
        Ok(_) => (),
        Err(_) => {
            log::error!("Cannot mount filesystem");
            return;
        },
    }
}
