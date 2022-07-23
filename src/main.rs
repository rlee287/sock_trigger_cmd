#![forbid(unsafe_code)]
use std::env::args_os;

use std::fs;

use std::collections::HashMap;

use log::{info, error, LevelFilter};
use simplelog::WriteLogger;

mod util;
use util::NonEmptyNoNullString;

fn main() -> Result<(), String> {
    let run_result = run();
    if let Err(ref e) = run_result {
        error!("{}", e);
    }
    run_result
}
fn run() -> Result<(), String> {
    let args: Vec<_> = args_os().collect();
    if args.len() != 3 {
        return Err("Usage: fifo_or_socket input_pipe config".to_owned());
    }

    let log_file = fs::OpenOptions::new().append(true)
        .open("/var/log/fifo_trigger_cmd.log")
        .map_err(|e| format!("Could not open log file: {}", e))?;
    WriteLogger::init(LevelFilter::Info, simplelog::Config::default(), log_file)
        .map_err(|e| format!("Could not init logging: {}", e))?;

    info!("Loading configuration file");
    let config_bytes = match fs::read(&args[2]) {
        Ok(val) => val,
        Err(e) => return Err(format!("Unable to read config: {}", e))
    };
    let config: HashMap<NonEmptyNoNullString, String> = serde_json::from_slice(&config_bytes).map_err(|e| format!("Config file must map string to string: {}", e))?;
    drop(config_bytes);

    if config.is_empty() {
        return Err("Config has no entries".to_owned());
    }
    let max_key_len = config.keys().map(|s| s.as_ref().len()).max().unwrap();

    let fifo_or_socket = fs::File::open(&args[1])
        .map_err(|e| format!("Could not open fifo_or_socket: {}", e))?;
    

    Ok(())
}
