use std::error;
use std::fmt;

/// A type to be used for the return of basic methods
pub type Return = Result<(), CerebroError>;

/// A struture used to report errors
#[derive(Debug)]
pub struct CerebroError {
    description: String
}

impl CerebroError {
    pub fn new(msg: &str) -> Self {
        Self {
            description: msg.to_string(),
        }
    }
}

impl fmt::Display for CerebroError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        return write!(f,"{}", self.description);
    }
}

impl error::Error for CerebroError {
    fn description(&self) -> &str {
        return &self.description;
    }
}

#[macro_export]
macro_rules! error {
    ($description: expr) => { Err(error::CerebroError::new($description)) }
}

#[macro_export]
macro_rules! success {
    () => {
        Ok(())
    }
}
