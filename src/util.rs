use serde::Deserialize;
use std::error::Error;
use std::borrow::Borrow;

use std::process::{Command, Output};

#[derive(Deserialize)]
#[serde(try_from = "String")]
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct NonEmptyNoNullString {
    inner: String
}
impl AsRef<str> for NonEmptyNoNullString {
    fn as_ref(&self) -> &str {
        self.inner.as_ref()
    }
}
impl Borrow<str> for NonEmptyNoNullString {
    fn borrow(&self) -> &str {
        self.inner.borrow()
    }
}
impl From<NonEmptyNoNullString> for String {
    fn from(nstr: NonEmptyNoNullString) -> Self {
        nstr.inner
    }
}
impl PartialEq<str> for NonEmptyNoNullString {
    fn eq(&self, other: &str) -> bool {
        self.inner == other
    }
}
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum TryIntoNonEmptyNoNullStringErr {
    Empty,
    HasNull(usize)
}
impl TryFrom<String> for NonEmptyNoNullString {
    type Error = TryIntoNonEmptyNoNullStringErr;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len()==0 {
            Err(TryIntoNonEmptyNoNullStringErr::Empty)
        } else if let Some(index) = value.as_bytes().iter().position(|c| *c==b'\x00') {
            Err(TryIntoNonEmptyNoNullStringErr::HasNull(index))
        } else {
            Ok(NonEmptyNoNullString {inner: value})
        }
    }
}
impl std::fmt::Display for TryIntoNonEmptyNoNullStringErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TryIntoNonEmptyNoNullStringErr::Empty =>
                f.write_str("Empty string"),
            TryIntoNonEmptyNoNullStringErr::HasNull(i) =>
                write!(f, "String has null at {}", i),
        }
    }
}
impl Error for TryIntoNonEmptyNoNullStringErr {}

#[derive(Debug)]
pub enum RunCmdError {
    InvalidShlex,
    CmdSpawnFailure(std::io::Error)
}
impl std::fmt::Display for RunCmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunCmdError::InvalidShlex =>
                f.write_str("Could not parse command"),
            RunCmdError::CmdSpawnFailure(e) =>
                write!(f, "Could not spawn command: {}", e),
        }
    }
}
impl Error for RunCmdError {}

pub fn run_cmd(cmd: &str) -> Result<Output, RunCmdError> {
    let cmd_args = shlex::split(cmd).ok_or(RunCmdError::InvalidShlex)?;

    let first_non_env_index = cmd_args.iter()
        .position(|s| !s.contains('=')).unwrap_or(0);
    let env_map = cmd_args[..first_non_env_index].iter()
        .map(|s| {
            let eq_pos = s.find('=').unwrap();
            (&s[..eq_pos], &s[eq_pos+1..])
        });
    let cmd_obj = Command::new(&cmd_args[first_non_env_index])
        .args(&cmd_args[first_non_env_index+1..])
        .env_clear()
        .envs(env_map)
        // Default of output() is null stdin and piped stdout
        .output();
    cmd_obj.map_err(|e| RunCmdError::CmdSpawnFailure(e))
}