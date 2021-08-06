use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;
use std::time;

use crate::config;
use crate::error;
use crate::events;
use crate::filesystem;

#[derive(Debug, PartialEq)]
pub enum Status
{
    Changed(String),
    Error,
    Ok,
}

pub trait Module: Send {
    fn name(&self) -> &str;

    fn start(&mut self, config: &config::ModuleConfig) -> error::CerebroResult;

    fn stop(&mut self) -> error::CerebroResult;

    fn is_running(&self) -> bool;

    fn fs_entries(&self) -> Vec<filesystem::FsEntry>;

    fn value(&self, inode: u64) -> String;

    fn set_value(&mut self, inode:u64, data: &[u8]);

    fn json(&self) -> String;

    fn shell(&self) -> String;
}

pub trait Data: Send {
    fn update(&mut self) -> Result<Status, error::CerebroError>;
}

pub struct Thread {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    stopper: Option<Mutex<Sender<()>>>,
    event_sender: Arc<Mutex<Sender<events::Events>>>,
}

impl Thread {
    pub fn new(event_sender: Arc<Mutex<Sender<events::Events>>>) -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
            stopper: None,
            event_sender: event_sender,
        }
    }

    pub fn start(
        &mut self,
        data: Arc<Mutex<dyn Data>>,
        timeout_s: Option<u64>) -> error::CerebroResult {

        // Check status
        if self.running.load(Ordering::SeqCst) {
            return Success!();
        }

        self.running.store(true, Ordering::SeqCst);

        // Check timeout
        let timeout_s = match timeout_s {
            Some(t) => t,
            None => return error!("No timeout given to the thread"),
        };

        // Get handle to stop the thread
        let (tx, rx): (Sender<()>, Receiver<()>) = channel();
        let sender = self.event_sender.clone();

        self.stopper = Some(Mutex::new(tx));

        // Spawn the thread
        self.handle = Some(thread::spawn(move || loop {
            let status: Status;

            {
                // Call update on the module's data
                let mut data = match data.lock() {
                    Ok(d) => d,
                    Err(_) => {
                        log::error!("Cannot lock module's data");
                        break;
                    },
                };

                status = match data.update() {
                    Ok(s) => s,
                    Err(e) => {
                        log::error!("Cannot update module: {}", e);
                        Status::Error
                    },
                };
            }

            // Check if the module has changed (then the thread needs to be
            // stopped)
            match status {
                Status::Changed(name) => {
                    let sender = match sender.lock() {
                        Ok(s) => s,
                        Err(_) => {
                            log::error!("Cannot lock event sender");
                            break;
                        },
                    };

                    match sender.send(events::Events::ModuleUpdated(name)) {
                        Ok(_) => (),
                        Err(_) => log::error!("Cannot send event"),
                    }

                    break;
                },

                _ => (),
            }

            // Check if a stop has been requested
            match rx.try_recv() {
                Ok(_) | Err(TryRecvError::Disconnected) => {
                    break;
                },

                Err(TryRecvError::Empty) => (),
            }

            // Wait a moment
            thread::sleep(time::Duration::from_secs(timeout_s));
        }));

        return Success!();
    }

    pub fn stop(&mut self) -> error::CerebroResult {
        // Send stop signal to the thread
        let stopper = match &self.stopper {
            Some(s) => s,
            None => return Success!(),
        };

        let stopper = match stopper.lock() {
            Ok(s) => s,
            Err(_) => return error!("Cannot lock stopper"),
        };

        match stopper.send(()) {
            Ok(_) => (),
            Err(_) => (), // If sender is closed this must means that the thread
                          // is already stopped
        }

        // Wait the thread to finish
        let handle = match self.handle.take() {
            Some(h) => h,
            None => return Success!(),
        };

        match handle.join() {
            Ok(_) => self.running.store(false, Ordering::SeqCst),
            Err(_) => return error!("Cannot join thread"),
        }

        return Success!();
    }

    pub fn is_running(&self) -> bool {
        return self.running.load(Ordering::SeqCst);
    }
}
