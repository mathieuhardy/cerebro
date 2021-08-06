use std::sync::mpsc::{channel, Receiver, Sender};

use std::sync::{Arc, Mutex};

use crate::events::Events;

#[derive(Debug)]
pub struct EventManager {
    rx: Arc<Mutex<Receiver<Events>>>,
    tx: Arc<Mutex<Sender<Events>>>,
}

impl EventManager {
    pub fn new() -> Self {
        let (tx, rx) = channel();

        Self {
            rx: Arc::new(Mutex::new(rx)),
            tx: Arc::new(Mutex::new(tx)),
        }
    }

    pub fn sender(&mut self) -> Arc<Mutex<Sender<Events>>> {
        return self.tx.clone();
    }

    pub fn receiver(&mut self) -> Arc<Mutex<Receiver<Events>>> {
        return self.rx.clone();
    }
}
