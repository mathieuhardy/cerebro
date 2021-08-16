use regex::Regex;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process;

use crate::error;

/// Type of trigger
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Create,
    Delete,
    Invalid,
    Update,
}

/// The structure used to store a trigger configuration
#[derive(Clone, Debug)]
pub struct Trigger {
    pub kind: Kind,
    pub path: String,

    command: String,
}

impl Trigger {
    pub fn new(kind: &str, path: &str, command: &str) -> Self {
        Self {
            kind: match kind {
                "C" => Kind::Create,
                "D" => Kind::Delete,
                "U" => Kind::Update,
                _ => Kind::Invalid,
            },
            path: path.to_string(),
            command: command.to_string(),
        }
    }

    pub fn execute(&self) -> error::CerebroResult {
        log::debug!("{} >>> {}", self.path, self.command);

        for command in self.command.split(";") {
            let mut binary = command.split(" ").collect::<Vec<&str>>();

            let args = binary.split_off(1);

            let output = match process::Command::new(binary[0])
                .args(args).output() {

                Ok(o) => o,
                Err(e) =>
                    return error!(&format!("Cannot execute command: {:?}", e)),
            };

            if !output.status.success() {
                return error!("Command is not successful");
            }
        }

        return Success!();
    }

    pub fn matches(&self, kind: Kind, path: &str) -> bool {
        if self.kind != kind {
            return false;
        }

        let re = match Regex::new(&self.path) {
            Ok(r) => r,
            Err(_) => {
                log::error!("Cannot build regex");
                return false;
            },
        };

        if re.is_match(path) {
            return true;
        }

        let re = match Regex::new(path) {
            Ok(r) => r,
            Err(_) => {
                log::error!("Cannot build regex");
                return false;
            },
        };

        return re.is_match(&self.path);
    }
}

/// Function used to load the triggers from a file
fn load_file<P: AsRef<Path>>(path: P)
    -> Result<Vec<Trigger>, error::CerebroError> {

    let mut triggers: Vec<Trigger> = Vec::new();

    // Open the file in read-only mode
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return error!("Cannot open trigger file"),
    };

    let re_line = Regex::new(r"^(C|D|U) ([^=]+)=(.*)").unwrap();

    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        let captures = match re_line.captures(&line) {
            Some(c) => c,
            None => {
                log::debug!("Invalid trigger: {:?}", line);
                continue;
            },
        };

        let kind = match captures.get(1) {
            Some(t) => t.as_str(),
            None => continue,
        };

        let path = match captures.get(2) {
            Some(p) => p.as_str(),
            None => continue,
        };

        let command = match captures.get(3) {
            Some(c) => c.as_str(),
            None => continue,
        };

        triggers.push(Trigger::new(kind, path, command));
    }

    return Ok(triggers);
}

/// Function used to load the triggers from a directory
pub fn load<P: AsRef<Path>>(path: P)
    -> Result<Vec<Trigger>, error::CerebroError> {

    let mut triggers: Vec<Trigger> = Vec::new();

    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => return Ok(triggers),
    };

    let re_file = match Regex::new(r"^.*\.triggers$") {
        Ok(r) => r,
        Err(_) => return error!("Cannot build regex"),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let p = entry.path();

        let p = match p.to_str() {
            Some(p) => p,
            None => continue,
        };

        if ! re_file.is_match(&p) {
            continue;
        }

        match load_file(p) {
            Ok(mut t) => triggers.append(&mut t),
            Err(_) => log::error!("Error loading triggers from {}", p),
        }
    }

    return Ok(triggers);
}

/// Function used to find all trigger that matches a pattern and execute them
pub fn find_all_and_execute<'a>(
    triggers: &'a Vec<Trigger>,
    kind: Kind,
    module: &str,
    name: &str) {

    for trigger in triggers.iter() {
        if ! trigger.matches(kind, &format!("/{}/{}", module, name)) {
            continue;
        }

        match trigger.execute() {
            Ok(_) => (),
            Err(e) => log::error!("{}", e),
        }
    }
}
