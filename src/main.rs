#![forbid(unsafe_code)]
use std::env::args_os;

use std::fs;
use std::path::PathBuf;
use std::io::ErrorKind;

use nix::unistd::Uid;
use nix::sys::stat::{fchmod, Mode};
use std::os::unix::io::AsRawFd;

use std::collections::HashMap;

use std::sync::Arc;

use tokio::runtime::Runtime;
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use tokio::net::{UnixListener, UnixStream};

use log::{debug, info, warn, error, log, Level, LevelFilter};
use flexi_logger::{Logger, FileSpec};
use flexi_logger::writers::{Syslog, SyslogWriter};

mod util;
use util::NonEmptyNoNullString;

mod run_cmd;

use std::ops::Deref;

async fn handle_connection(config: impl Deref<Target=HashMap<NonEmptyNoNullString, Vec<String>>>, mut stream: UnixStream) {
    debug!("Establishing connection");
    let max_key_len = config.keys().map(|s| s.as_ref().len()).max().unwrap();
    // TODO: add shift-reg style buffering
    let mut socket_byte_buf: [u8; 1] = [0x00];

    'cmd_loop: loop {
        let mut key_vec: Vec<u8> = Vec::with_capacity(max_key_len);
        while key_vec.len() <= max_key_len {
            match stream.read(&mut socket_byte_buf).await {
                Ok(1) => {
                    // Null byte scanning works because UTF-8 does not have nulls
                    if socket_byte_buf[0] == b'\x00' {
                        break;
                    }
                    key_vec.extend(socket_byte_buf);
                },
                Ok(0) => {
                    break 'cmd_loop;
                },
                Ok(_) => unreachable!(),
                Err(e) => {
                    if e.kind() == ErrorKind::Interrupted {
                        continue;
                    }
                    error!("Could not read from socket: {}", e);
                    // Go ahead and wipe the buffer
                    continue 'cmd_loop;
                }
            };
        }
        let key_str = match std::str::from_utf8(&key_vec) {
            Ok(s) => s,
            Err(_) => {
                // Wouldn't match our keys anyways
                warn!("Received non-matching key with invalid utf8 {}", String::from_utf8_lossy(&key_vec));
                if let Err(e) = stream.write_all(b"X").await {
                    error!("Could not write to socket: {}", e);
                }
                continue;
            }
        };
        match config.get(key_str) {
            Some(cmd) => {
                match run_cmd::run_cmd(cmd).await {
                    Ok(output) => {
                        let log_output_level = match output.status.code() {
                            Some(exit_code) => {
                                let finish_level = match exit_code {
                                    0 => Level::Info,
                                    _ => Level::Warn
                                };
                                log!(finish_level, "Command {:?} exited with code {}", cmd, exit_code);
                                if let Err(e) = stream.write_all(&[b'C', (exit_code%256) as u8]).await {
                                    error!("Could not write to socket: {}", e);
                                }
                                match exit_code {
                                    0 => Level::Debug,
                                    _ => Level::Warn
                                }
                            },
                            None => {
                                warn!("Command {:?} terminated by signal", cmd);
                                if let Err(e) = stream.write_all(b"S").await {
                                    error!("Could not write to socket: {}", e);
                                }
                                Level::Warn
                            }
                        };
                        log!(log_output_level, "stdout for {:?}:\n{}", cmd, String::from_utf8_lossy(&output.stdout));
                        log!(log_output_level, "stderr for {:?}:\n{}", cmd, String::from_utf8_lossy(&output.stderr));
                    },
                    Err(e) => {
                        error!("Error starting command: {}", e);
                        if let Err(e) = stream.write_all(b"F").await {
                            error!("Could not write to socket: {}", e);
                        }
                    }
                }
            },
            None => {
                warn!("Received non-matching key {}", key_str);
                if let Err(e) = stream.write_all(b"X").await {
                    error!("Could not write to socket: {}", e);
                }
                continue;
            }
        }
    }
    debug!("Closing connection");
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
        return Err("Usage: sock_trigger_cmd socket_loc config".to_owned());
    }

    let log_path = match Uid::effective().is_root() {
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
    let config_iter = serde_json::from_slice::<HashMap<NonEmptyNoNullString, String>>(&config_bytes)
        .map_err(|e| format!("Config file must map string to string: {}", e))?
        .into_iter()
        .map(|(k, v)| {
            let shlexed = match shlex::split(&v) {
                Some(vec) => Ok(vec),
                None => Err(v)
            };
            (k, shlexed)
        });
    drop(config_bytes);

    let config = {
        let mut config_map = HashMap::new();
        for (k, v) in config_iter {
            match v {
                Ok(vec) => {
                    config_map.insert(k,vec);
                }
                Err(s) => {
                    return Err(format!("Command {} could not be parsed", s));
                }
            }
        }
        config_map
    };

    if config.is_empty() {
        return Err("Config has no entries".to_owned());
    }

    debug!("Removing old socket file if it exists");
    let path = PathBuf::from(&args[1]);
    if path.exists() {
        if path.is_file() && path.metadata().unwrap().len() > 0 {
            return Err("Refusing to remove nonempty file at socket path".to_owned());
        }
        // Do this to avoid pulling in tokio::fs
        fs::remove_file(path).unwrap();
    }

    info!("Starting async runtime");
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let socket = UnixListener::bind(&args[1])
            .map_err(|e| format!("Could not open socket: {}", e))?;
        fchmod(socket.as_raw_fd(),  Mode::from_bits(0o660).unwrap())
            .map_err(|e| format!("Could not set socket permissions: {}", e))?;

        info!("Starting processing loop");
        let config_arc = Arc::new(config);
        loop {
            let stream = match socket.accept().await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    warn!("Error with receiving connection: {}", e);
                    continue;
                }
            };
            let config_arc = config_arc.clone();
            rt.spawn(handle_connection(config_arc, stream));
        }
        // Need unreachable return to infer async closure return type
        Ok::<_, String>(())
    })?;
    /*thread::scope(|t| {
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
    }).unwrap();*/

    Ok(())
}
