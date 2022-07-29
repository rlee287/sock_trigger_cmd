#![forbid(unsafe_code)]
use std::env::args_os;

use std::fs;
use std::path::PathBuf;
use std::io::{Read, Write, ErrorKind};
#[cfg(target_family = "unix")]
use std::os::unix::net::{UnixListener, UnixStream};

use std::collections::HashMap;

use crossbeam_utils::thread;

use log::{debug, info, warn, error, log, Level, LevelFilter};
use flexi_logger::{Logger, FileSpec};
use flexi_logger::writers::{Syslog, SyslogWriter};
//use simplelog::{WriteLogger, CombinedLogger, SimpleLogger};

mod util;
use util::NonEmptyNoNullString;

fn handle_connection(config: &HashMap<NonEmptyNoNullString, String>, stream: UnixStream) {
    debug!("Thread spawned for new connection");
    let max_key_len = config.keys().map(|s| s.as_ref().len()).max().unwrap();
    let mut socket_bytes = (&stream).bytes();

    'cmd_loop: loop {
        let mut key_vec: Vec<u8> = Vec::with_capacity(max_key_len);
        while key_vec.len() <= max_key_len {
            let next_byte = match socket_bytes.next() {
                Some(Ok(b)) => b,
                Some(Err(e)) => {
                    if e.kind() == ErrorKind::Interrupted {
                        continue;
                    }
                    error!("Could not read from socket: {}", e);
                    // Go ahead and wipe the buffer
                    continue 'cmd_loop;
                }
                None => {
                    break 'cmd_loop;
                }
            };
            // Null byte scanning works because UTF-8 does not have nulls
            if next_byte == b'\x00' {
                break;
            }
            key_vec.push(next_byte);
        }
        let key_str = match std::str::from_utf8(&key_vec) {
            Ok(s) => s,
            Err(_) => {
                // Wouldn't match our keys anyways
                warn!("Received non-matching key with invalid utf8 {}", String::from_utf8_lossy(&key_vec));
                if let Err(e) = (&stream).write_all(b"X") {
                    error!("Could not write to socket: {}", e);
                }
                continue;
            }
        };
        match config.get(key_str) {
            Some(cmd) => {
                match util::run_cmd(cmd) {
                    Ok(output) => {
                        let log_output_level = match output.status.code() {
                            Some(exit_code) => {
                                let finish_level = match exit_code {
                                    0 => Level::Info,
                                    _ => Level::Warn
                                };
                                log!(finish_level, "Command {} exited with code {}", cmd, exit_code);
                                if let Err(e) = (&stream).write_all(&[b'C', (exit_code%256) as u8]) {
                                    error!("Could not write to socket: {}", e);
                                }
                                match exit_code {
                                    0 => Level::Debug,
                                    _ => Level::Warn
                                }
                            },
                            None => {
                                warn!("Command {} terminated by signal", cmd);
                                if let Err(e) = (&stream).write_all(b"S") {
                                    error!("Could not write to socket: {}", e);
                                }
                                Level::Warn
                            }
                        };
                        log!(log_output_level, "stdout for {}:\n{}", cmd, String::from_utf8_lossy(&output.stdout));
                        log!(log_output_level, "stderr for {}:\n{}", cmd, String::from_utf8_lossy(&output.stderr));
                    },
                    Err(e) => {
                        error!("Error starting command: {}", e);
                        if let Err(e) = (&stream).write_all(b"F") {
                            error!("Could not write to socket: {}", e);
                        }
                    }
                }
            },
            None => {
                warn!("Received non-matching key {}", key_str);
                if let Err(e) = (&stream).write_all(b"X") {
                    error!("Could not write to socket: {}", e);
                }
                continue;
            }
        }
    }
    debug!("Thread exiting")
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
        eprintln!("Usage: sock_trigger_cmd socket_loc config");
        return Err(String::new());
    }

    /*let log_file = fs::OpenOptions::new().create(true).append(true)
        .open("./sock_trigger_cmd.log")
        .map_err(|e| format!("Could not open log file: {}", e))?;*/
    let log_path = match nix::unistd::Uid::effective().is_root() {
        true => "/var/log/sock_trigger_cmd.log".to_owned(),
        false => std::env::var("HOME").unwrap()+"/sock_trigger_cmd.log"
    };
    let _logger = Logger::try_with_env_or_str("debug")
        .map_err(|e| format!("Could not initialize logging: {}", e))?
        .o_append(true)
        .log_to_file_and_writer(FileSpec::try_from(log_path).unwrap(),
            SyslogWriter::try_new(flexi_logger::writers::SyslogFacility::SystemDaemons,
                None, LevelFilter::Info,
                "sock_trigger_cmd".to_owned(),
                Syslog::try_datagram("/dev/log").unwrap()
            ).unwrap()
        )
        .duplicate_to_stdout(flexi_logger::Duplicate::Info)
        .format_for_files(flexi_logger::opt_format)
        .format_for_stdout(flexi_logger::opt_format)
        .start()
        .map_err(|e| format!("Could not initialize logging: {}", e))?;

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

    debug!("Removing old socket file if it exists");
    let path = PathBuf::from(&args[1]);
    if path.exists() {
        if path.is_file() && path.metadata().unwrap().len() > 0 {
            return Err("Refusing to remove nonempty file at socket path".to_owned());
        }
        fs::remove_file(path).unwrap();
    }

    info!("Opening socket");
    // TODO: allow for a regular TCP socket read as well
    let socket = UnixListener::bind(&args[1])
        .map_err(|e| format!("Could not open socket: {}", e))?;

    info!("Starting processing loop");
    thread::scope(|t| {
        for conn_result in socket.incoming() {
            match conn_result {
                Ok(conn) => {
                    t.spawn(|_| handle_connection(&config, conn));
                },
                Err(e) => {
                    debug!("Error with receiving connection: {}", e);
                    break;
                }
            }
        }
    }).unwrap();

    Ok(())
}
