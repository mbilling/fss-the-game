use std::fmt;

/// A scenario, curve, plan or interaction-log problem, naming the file and key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub file: String,
    pub key: String,
    pub msg: String,
}

impl LoadError {
    pub fn new(file: impl Into<String>, key: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            key: key.into(),
            msg: msg.into(),
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.key.is_empty() {
            write!(f, "{}: {}", self.file, self.msg)
        } else {
            write!(f, "{}: {}: {}", self.file, self.key, self.msg)
        }
    }
}

impl std::error::Error for LoadError {}
