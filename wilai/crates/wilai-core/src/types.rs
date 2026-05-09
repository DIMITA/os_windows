use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Read,
    Write,
    Network,
    Destructive,
    Privileged,
    Pentest,
    Keyring,
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Category::Read => "read",
            Category::Write => "write",
            Category::Network => "network",
            Category::Destructive => "destructive",
            Category::Privileged => "privileged",
            Category::Pentest => "pentest",
            Category::Keyring => "keyring",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    None,
    Low,
    Medium,
    High,
    Critical,
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Risk::None => "none",
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
            Risk::Critical => "critical",
        };
        f.write_str(s)
    }
}

pub type SessionId = String;
pub type TurnId = String;
pub type ToolCallId = String;

pub fn new_ulid() -> String {
    ulid::Ulid::new().to_string()
}
