#![forbid(unsafe_code)]
use std::env::args_os;

use std::fs;
use std::io::{Read, ErrorKind};
#[cfg(target_family = "unix")]
use std::os::unix::fs::FileTypeExt;

use std::collections::HashMap;

use std::sync::atomic::{Ordering, AtomicBool};

use log::{info, warn, error, log, Level, LevelFilter};
use simplelog::{WriteLogger, CombinedLogger, SimpleLogger};

mod util;
use util::NonEmptyNoNullString;

static PROCESS_INPUT: AtomicBool = AtomicBool::new(true);

#[cfg(debug_assertions)]
const LOG_LEVEL: LevelFilter = LevelFilter::Debug;
#[cfg(not(debug_assertions))]
const LOG_LEVEL: LevelFilter = LevelFilter::Info;

fn graceful_stop() {
    info!("Received Ctrl-C, stopping");
    PROCESS_INPUT.store(false, Ordering::Release);
}

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
        eprintln!("Usage: fifo_or_socket input_pipe config");
        return Err(String::new());
    }

    let log_file = fs::OpenOptions::new().create(true).append(true)
        .open("./fifo_trigger_cmd.log")
        .map_err(|e| format!("Could not open log file: {}", e))?;
    CombinedLogger::init(vec![
        WriteLogger::new(LOG_LEVEL, simplelog::Config::default(), log_file),
        SimpleLogger::new(LOG_LEVEL, simplelog::Config::default())
    ]).map_err(|e| format!("Could not init logging: {}", e))?;

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

    info!("Opening fifo_or_socket");
    // TODO: allow for a regular TCP socket read as well
    let fifo_or_socket = fs::File::open(&args[1])
        .map_err(|e| format!("Could not open fifo_or_socket: {}", e))?;

    let fifo_or_socket_type = fifo_or_socket.metadata().unwrap().file_type();
    if !(fifo_or_socket_type.is_fifo() || fifo_or_socket_type.is_socket()) {
        return Err("fifo_or_socket must be fifo or socket".to_owned());
    }
    let mut fifo_or_socket_bytes = fifo_or_socket.bytes();

    ctrlc::set_handler(|| graceful_stop()).unwrap();

    info!("Starting processing loop");
    'cmd_loop: while PROCESS_INPUT.load(Ordering::Acquire) {
        let mut key_vec: Vec<u8> = Vec::with_capacity(max_key_len);
        while key_vec.len() <= max_key_len {
            let next_byte: Option<u8> = match fifo_or_socket_bytes.next() {
                Some(Ok(b)) => Some(b),
                Some(Err(e)) => {
                    if e.kind() == ErrorKind::Interrupted {
                        continue;
                    }
                    error!("Could not read from fifo_or_socket: {}", e);
                    // Go ahead and wipe the buffer
                    continue 'cmd_loop;
                }
                None => {
                    // FIFO returns EOF when write end is closed
                    // Write end closed -> discard the buffer
                    key_vec.clear();
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    None
                    //PROCESS_INPUT.store(false, Ordering::Release);
                    //break;
                }
            };
            if let Some(next_byte) = next_byte {
                // Null byte scanning works because UTF-8 does not have nulls
                if next_byte == b'\x00' {
                    break;
                }
                key_vec.push(next_byte);
            }
        }
        let key_str = match std::str::from_utf8(&key_vec) {
            Ok(s) => s,
            Err(_) => {
                // Wouldn't match our keys anyways
                error!("Received non-matching key with invalid utf8 {}", String::from_utf8_lossy(&key_vec));
                continue;
            }
        };
        match config.get(key_str) {
            Some(cmd) => {
                match util::run_cmd(cmd) {
                    Ok(output) => {
                        let log_output_level = match output.status.code() {
                            Some(0) => {
                                info!("Command {} exited with code 0", cmd);
                                Level::Debug
                            },
                            Some(e) => {
                                warn!("Command {} exited with code {}", cmd, e);
                                Level::Warn
                            },
                            None => {
                                warn!("Command {} terminated by signal", cmd);
                                Level::Warn
                            }
                        };
                        log!(log_output_level, "stdout for {}:\n{}", cmd, String::from_utf8_lossy(&output.stdout));
                        log!(log_output_level, "stderr for {}:\n{}", cmd, String::from_utf8_lossy(&output.stderr));
                    },
                    Err(e) => {
                        error!("Error starting command: {}", e);
                    }
                }
            },
            None => {
                warn!("Received non-matching key {}", key_str);
                continue;
            }
        }
    }

    Ok(())
}
