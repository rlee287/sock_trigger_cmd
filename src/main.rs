#![forbid(unsafe_code)]
use argh::FromArgs;

use std::fs;
use std::path::PathBuf;
use std::io::ErrorKind;

use nix::unistd::Uid;
use nix::sys::stat::{fchmod, Mode};
use std::os::unix::io::AsRawFd;

use std::collections::HashMap;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::runtime::Runtime;
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::select;
use tokio::sync::mpsc::{channel, Sender};

use std::os::unix::process::ExitStatusExt;

use log::{debug, info, warn, error, log, Level, LevelFilter};
use flexi_logger::{Logger, FileSpec};
use flexi_logger::writers::{Syslog, SyslogWriter};

mod util;
use util::NonEmptyNoNullString;

mod run_cmd;

use std::ops::Deref;

static IS_HALTING: AtomicBool = AtomicBool::new(false);

async fn handle_connection(config: impl Deref<Target=HashMap<NonEmptyNoNullString, Vec<String>>>,
        mut stream: UnixStream, _send_token: Sender<()>) {
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
                                let ret_chars = [b'C', (exit_code%256) as u8];
                                if let Err(e) = stream.write_all(&ret_chars).await {
                                    error!("Could not write to socket: {}", e);
                                }
                                match exit_code {
                                    0 => Level::Debug,
                                    _ => Level::Warn
                                }
                            },
                            None => {
                                let sig = output.status.signal().unwrap();
                                warn!("Command {:?} terminated by signal {}", cmd, sig);
                                let ret_chars = [b'S', (sig%256) as u8];
                                if let Err(e) = stream.write_all(&ret_chars).await {
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
        if IS_HALTING.load(Ordering::Acquire) {
            break;
        }
    }
    debug!("Closing connection");
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(FromArgs)]
#[argh(description = "Start server to run commands based on keys from Unix domain socket")]
struct CmdArgs {
    #[argh(switch, short = 'q')]
    #[argh(description = "do not log to stdout")]
    no_stdout_logs: bool,
    #[argh(positional)]
    #[argh(description = "location to create socket at")]
    socket_location: PathBuf,
    #[argh(positional)]
    #[argh(description = "location for config file")]
    config_location: PathBuf
}

fn main() -> Result<(), String> {
    let run_result = run();
    if let Err(ref e) = run_result {
        error!("{}", e);
    }
    run_result
}
fn run() -> Result<(), String> {
    let args: CmdArgs = argh::from_env();

    let log_path = match Uid::effective().is_root() {
        true => "/var/log/sock_trigger_cmd.log".to_owned(),
        false => std::env::var("HOME").unwrap()+"/sock_trigger_cmd.log"
    };
    let _logger_handle = {
        let mut logger = Logger::try_with_env_or_str("debug")
            .map_err(|e| format!("Could not initialize logging: {}", e))?
            .o_append(true)
            .log_to_file_and_writer(FileSpec::try_from(log_path).unwrap(),
                SyslogWriter::try_new(flexi_logger::writers::SyslogFacility::SystemDaemons,
                    None, LevelFilter::Info,
                    "sock_trigger_cmd".to_owned(),
                    Syslog::try_datagram("/dev/log").unwrap()
                ).unwrap()
            )
            .format_for_files(flexi_logger::opt_format);
        if !args.no_stdout_logs {
            logger = logger.duplicate_to_stdout(flexi_logger::Duplicate::Info)
                .format_for_stdout(flexi_logger::opt_format)
        }
        logger.start()
            .map_err(|e| format!("Could not initialize logging: {}", e))?
    };

    info!("Loading configuration file");
    let config_bytes = match fs::read(args.config_location) {
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
    if args.socket_location.exists() {
        if args.socket_location.is_file() && args.socket_location.metadata().unwrap().len() > 0 {
            return Err("Refusing to remove nonempty file at socket path".to_owned());
        }
        fs::remove_file(&args.socket_location).unwrap();
    }

    info!("Starting async runtime");
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let socket = UnixListener::bind(&args.socket_location)
            .map_err(|e| format!("Could not open socket: {}", e))?;
        fchmod(socket.as_raw_fd(),  Mode::from_bits(0o660).unwrap())
            .map_err(|e| format!("Could not set socket permissions: {}", e))?;

        info!("Starting processing loop");
        let config_arc = Arc::new(config);
        let (send, mut recv) = channel(1);
        loop {
            select! {
                ctrl_c_res = tokio::signal::ctrl_c() => match ctrl_c_res {
                    Ok(()) => {
                        info!("Received Ctrl-C, finishing current tasks");
                        IS_HALTING.store(true, Ordering::Release);
                        break;
                    },
                    Err(e) => {
                        return Err(format!("Could not handle Ctrl-C: {}", e));
                    }
                },
                stream_res = socket.accept() => {
                    let stream = match stream_res {
                        Ok((stream, _)) => stream,
                        Err(e) => {
                            warn!("Error with receiving connection: {}", e);
                            continue;
                        }
                    };
                    let config_arc = config_arc.clone();
                    rt.spawn(handle_connection(config_arc, stream, send.clone()));
                }
            };
        }
        drop(send);
        let _ = recv.recv().await;

        Ok::<_, String>(())
    })?;

    info!("Exiting");
    Ok(())
}
