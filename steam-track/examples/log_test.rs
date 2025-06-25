// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

//! Basic test case using the steam_track library
//!
//! This application creates multiple threads creating "load" events and sending
//! them to the master thread which consumes them.
//!
//! This tests the structured nature of the log output with each event being
//! created within a context.
//!
//! # Examples
//!
//! Generate textual output to the console using:
//! ```
//!   ./target/debug/log_test --log-level debug --enable-trace --log-file -
//! ```
//!
//! Or create a Cap'n Proto output using:
//! ```
//!   ./target/debug/log_test --log-level trace --enable-trace --trace-file trace.bin
//! ```
//!
//! And the output Cap'n Proto file created can be inspected using:
//!
//! ```
//!   capnp convert packed:text steam-track/schemas/steam_track.capnp Event < foo.bin
//! ```

use std::io::BufWriter;
use std::net::TcpStream;
use std::sync::{Arc, mpsc};
use std::{fs, io, thread};

use steam_config::multi_source_config;
use steam_track::entity::{Entity, toplevel};
use steam_track::tracker::{CapnProtoTracker, EntityManager, TextTracker, Tracker};
use steam_track::{Writer, create_and_track_tag, debug, info, trace};

/// Command-line arguments.
#[multi_source_config]
#[command(about = "Logging test application.")]
struct Config {
    /// Configure the logging level for the log messages
    #[arg(long)]
    log_level: Option<String>,

    /// Enable trace events
    #[arg(long)]
    enable_trace: Option<bool>,

    /// Specify a log file to write text log/trace to.
    ///
    /// Use '-' to write to stdout. If left blank the Cap'n Proto trace output
    /// will be produced instead.
    #[arg(short = 'l', long = "log-file")]
    log_file: Option<String>,

    /// Will be ignored if `--log-file` or `--server` are specified
    #[arg(short = 't', long = "trace-file")]
    trace_file: Option<String>,

    /// Server address of the form IP:PORT to send Cap'n Proto trace to.
    ///
    /// Will take priority over `--trace-file`
    #[arg(short, long)]
    server: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: Some("warn".to_string()),
            enable_trace: Some(Default::default()),
            log_file: Some(String::new()),
            trace_file: Some(String::new()),
            server: Some(String::new()),
        }
    }
}

fn load(
    entity: &Entity,
    address: &mut u32,
    value: &mut u64,
    tx_id: &mpsc::Sender<(u64, steam_track::Tag)>,
) {
    let load_tag = create_and_track_tag!(entity);
    let load_value = *address as u64 | *value;
    let num_bytes = 64;

    // Trace that the load happened
    trace!(entity ; "Load: {}, {}, {}", num_bytes, *address, load_value);

    // Send to the master for processing
    tx_id.send((load_value, load_tag)).unwrap();

    *address += 4;
    *value += 1;
}

fn main() -> io::Result<()> {
    let config = Config::parse_all_sources();

    let default_entity_level = steam_track::str_to_level(&config.log_level.unwrap());

    let tracker: Tracker = if config.log_file.as_ref().unwrap().is_empty() {
        // Use a Capnp tracker
        let bin_writer: Writer = if config.server.as_ref().unwrap().is_empty() {
            Box::new(BufWriter::new(fs::File::create(
                config.trace_file.unwrap().clone(),
            )?))
        } else {
            Box::new(BufWriter::new(
                TcpStream::connect(config.server.unwrap()).unwrap(),
            ))
        };
        let entity_manger = EntityManager::new(default_entity_level);
        Arc::new(CapnProtoTracker::new(entity_manger, bin_writer))
    } else {
        // Use a textual output
        let txt_writer: Writer = if config.log_file.as_ref().unwrap() == "-" {
            Box::new(io::stdout())
        } else {
            Box::new(fs::File::create(config.log_file.unwrap())?)
        };
        let entity_manger = EntityManager::new(default_entity_level);
        Arc::new(TextTracker::new(entity_manger, txt_writer))
    };

    let top = toplevel(&tracker, "top");

    let num_threads = 10;
    let num_words = 5;

    // Create channel for loaded data and their Tags
    let (tx_id, rx_id) = mpsc::channel::<(u64, steam_track::Tag)>();

    // Launch a number of workers to 'load' data and send it to the main thread
    let mut threads = Vec::new();
    for n in 0..num_threads {
        let tx_id = tx_id.clone();
        let top = top.clone();
        threads.push(thread::spawn(move || {
            // Create the thread Id as a child of the simulator
            let thread = Entity::new(&top, format!("thread{n}").as_str());
            let mut address = 0x40000;
            let mut value = thread.tag.0;

            for _i in 0..num_words {
                load(&thread, &mut address, &mut value, &tx_id);
            }
        }));
    }

    // Receive and print log all loaded data
    for _i in 0..num_threads {
        for _j in 0..num_words {
            let (value, tag) = rx_id.recv().unwrap();
            info!(top ; "Received {:#x}, {}", value, tag);
        }
    }

    // Test with location only
    debug!(top ; "Waiting for threads to finish");
    for thread in threads {
        let _ = thread.join();
    }

    debug!(top ; "All done - exiting");
    Ok(())
}
