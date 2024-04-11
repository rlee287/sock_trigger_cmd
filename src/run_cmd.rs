use tokio::process::Command;
use std::process::Output;

use std::ffi::{OsStr, OsString};

/// Runs the tokenized passed-in command, separating out env vars first
pub async fn run_cmd(cmd_args: &Vec<String>) -> Result<Output, std::io::Error> {
    let first_non_env_index = cmd_args.iter()
        .position(|s| !s.contains('=')).unwrap_or(0);
    let parsed_env_map = cmd_args[..first_non_env_index].iter()
        .map(|s| {
            let eq_pos = s.find('=').unwrap();
            (&s[..eq_pos], &s[eq_pos+1..])
        })
        .map(|(s1, s2)| (OsStr::new(s1), OsString::from(s2)));
    // Preserve $HOME, $PATH, $USER, $SHELL, and $TERM if they exist
    let preserved_env_map = ["HOME", "PATH", "USER", "SHELL", "TERM"].iter()
        .filter_map(|s| {
            std::env::var_os(s).map(|env_var| (OsStr::new(s), env_var))
        });

    let cmd_obj = Command::new(&cmd_args[first_non_env_index])
        .args(&cmd_args[first_non_env_index+1..])
        .env_clear()
        // Chain parsed second so that it can override the preserved env vars
        .envs(preserved_env_map.chain(parsed_env_map))
        // Default of output() is null stdin and piped stdout
        .output()
        .await;
    cmd_obj
}