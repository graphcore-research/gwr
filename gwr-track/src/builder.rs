// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Library functions to build trackers as defined by the user.

use std::io::BufWriter;
use std::rc::Rc;
use std::{fs, io};

use clap::Args;

use crate::tracker::multi_tracker::MultiTracker;
#[cfg(feature = "perfetto")]
use crate::tracker::perfetto::PerfettoTracker;
use crate::tracker::{CapnProtoTracker, EntityManager, TextTracker, TrackConfigError};
use crate::{Tracker, Writer};

/// Standard command-line arguments for tracker configuration.
#[derive(Clone, Debug, Args)]
pub struct TrackerArgs {
    /// Enable logging to the console.
    #[arg(long, default_value = "false")]
    pub stdout: bool,

    /// Level of log message to display.
    #[arg(long, default_value = "Info")]
    pub stdout_level: log::Level,

    /// Set a regular expression for which entities should have logging level
    /// set to `--stdout-level`. Others will have level set to `Error`.
    #[arg(long, default_value = "")]
    pub stdout_filter_regex: String,

    /// Enable logging to binary file used by `gwr-spotter`.
    #[arg(long, default_value = "false")]
    pub binary: bool,

    /// Level of binary trace events to record.
    #[arg(long, default_value = "Trace")]
    pub binary_level: log::Level,

    /// Set a regular expression for which entities should have binary output
    /// level set to `--binary-level`. Others will have level set to `Error`.
    #[arg(long, default_value = "")]
    pub binary_filter_regex: String,

    /// The filename binary trace output is written to.
    #[arg(long, default_value = "trace.bin")]
    pub binary_file: String,

    /// Enable logging to Perfetto file used by `gwr-spotter`.
    #[cfg(feature = "perfetto")]
    #[arg(long, default_value = "false")]
    pub perfetto: bool,

    /// Level of Perfetto trace events to record.
    #[cfg(feature = "perfetto")]
    #[arg(long, default_value = "Trace")]
    pub perfetto_level: log::Level,

    /// Set a regular expression for which entities should have Perfetto output
    /// level set to `--perfetto-level`. Others will have level set to `Error`.
    #[cfg(feature = "perfetto")]
    #[arg(long, default_value = "")]
    pub perfetto_filter_regex: String,

    /// The filename Perfetto trace output is written to.
    #[cfg(feature = "perfetto")]
    #[arg(long, default_value = "trace.pftrace")]
    pub perfetto_file: String,

    /// Enable monitoring at the specified number of clock ticks.
    #[arg(long)]
    pub monitor_window_ticks: Option<u64>,

    /// Set a regular expression for which ports should have monitors enabled.
    #[arg(long, default_value = "")]
    pub monitor_filter_regex: String,
}

impl TrackerArgs {
    /// Return whether any tracker output has been explicitly requested.
    #[must_use]
    pub fn tracking_requested(&self) -> bool {
        let requested = self.stdout || self.binary;
        #[cfg(feature = "perfetto")]
        let requested = requested || self.perfetto;
        requested
    }

    /// Return whether any configured tracker will emit messages at `level`.
    #[must_use]
    pub fn level_enabled(&self, level: log::Level) -> bool {
        let shown = (self.stdout && self.stdout_level >= level)
            || (self.binary && self.binary_level >= level);
        #[cfg(feature = "perfetto")]
        let shown = shown || (self.perfetto && self.perfetto_level >= level);
        shown
    }

    /// Ensure that if the specified feature is enabled then a tracker will be
    /// showing messages of that level
    pub fn ensure_visiblity(&mut self, feature: bool, feature_name: &str, level: log::Level) {
        if feature && !self.level_enabled(level) {
            self.stdout = true;
            if self.stdout_level < log::Level::Info {
                self.stdout_level = log::Level::Info;
            }
            eprintln!(
                "WARNING: `{feature_name}` emits {level} messages, but no tracker is configured to show that level. Enabling stdout at {}.",
                self.stdout_level
            );
        }
    }

    /// Convert these command-line arguments into a [`TrackersConfig`].
    #[must_use]
    pub fn trackers_config(&self) -> TrackersConfig<'_> {
        TrackersConfig {
            stdout: TrackerConfig {
                enable: self.stdout,
                level: self.stdout_level,
                filter_regex: &self.stdout_filter_regex,
                file: None,
            },
            binary: TrackerConfig {
                enable: self.binary,
                level: self.binary_level,
                filter_regex: &self.binary_filter_regex,
                file: Some(&self.binary_file),
            },
            #[cfg(feature = "perfetto")]
            perfetto: TrackerConfig {
                enable: self.perfetto,
                level: self.perfetto_level,
                filter_regex: &self.perfetto_filter_regex,
                file: Some(&self.perfetto_file),
            },
            monitors: MonitorsConfig {
                enable: self.monitor_window_ticks.is_some(),
                window_size_ticks: self.monitor_window_ticks.unwrap_or(0),
                filter_regex: &self.monitor_filter_regex,
            },
        }
    }
}

/// Configuration options for an individual tracker.
pub struct TrackerConfig<'a> {
    /// Enable this tracker.
    pub enable: bool,

    /// Set the level at which this tracker should be enabled.
    pub level: log::Level,

    /// A regular expression to match which entities should have this level
    /// applied.
    pub filter_regex: &'a str,

    /// If required, the name of the file to which the tracker will write.
    pub file: Option<&'a str>,
}

impl Default for TrackerConfig<'_> {
    fn default() -> Self {
        Self {
            enable: true,
            level: log::Level::Warn,
            filter_regex: "",
            file: None,
        }
    }
}

/// Configuration options for monitoring.
#[derive(Default)]
pub struct MonitorsConfig<'a> {
    /// Enable monitoring.
    pub enable: bool,

    /// Window size in clock ticks to process monitoring.
    pub window_size_ticks: u64,

    /// Regular expression for which entities should have monitoring enabled.
    pub filter_regex: &'a str,
}

/// Configuration options for all tracking/monitoring.
pub struct TrackersConfig<'a> {
    /// Configuration for stdout.
    pub stdout: TrackerConfig<'a>,

    /// Configuration for binary trace file.
    pub binary: TrackerConfig<'a>,

    #[cfg(feature = "perfetto")]
    /// Configuration for perfetto trace file.
    pub perfetto: TrackerConfig<'a>,

    /// Configuration for monitoring.
    pub monitors: MonitorsConfig<'a>,
}

/// Create a tracker that prints to stdout
///
/// The user can pass a filter regular expression which will set the level only
/// for matching Entities and set all other Entities to only emit errors.
fn build_stdout_tracker(
    config: &TrackerConfig,
    monitors: &MonitorsConfig,
) -> Result<Tracker, TrackConfigError> {
    let default_level = if config.filter_regex.is_empty() {
        config.level
    } else {
        log::Level::Error
    };

    let mut entity_manager = EntityManager::new(default_level);
    if !config.filter_regex.is_empty() {
        entity_manager.add_entity_level_filter(config.filter_regex, config.level)?;
    }

    if monitors.enable {
        entity_manager
            .set_monitor_window_size_for(monitors.filter_regex, monitors.window_size_ticks)?;
    }

    let stdout_writer = Box::new(std::io::BufWriter::new(io::stdout()));
    Ok(Rc::new(TextTracker::new(entity_manager, stdout_writer)))
}

/// Same as the text tracker (see build_stdout_tracker) except will generate a
/// binary file.
fn build_binary_tracker(
    config: &TrackerConfig,
    monitors: &MonitorsConfig,
) -> Result<Tracker, TrackConfigError> {
    let default_level = if config.filter_regex.is_empty() {
        config.level
    } else {
        log::Level::Error
    };
    let mut entity_manager = EntityManager::new(default_level);
    if !config.filter_regex.is_empty() {
        entity_manager.add_entity_level_filter(config.filter_regex, config.level)?;
    }

    if monitors.enable {
        entity_manager
            .set_monitor_window_size_for(monitors.filter_regex, monitors.window_size_ticks)?;
    }

    let bin_writer: Writer = Box::new(BufWriter::new(
        fs::File::create(config.file.unwrap()).unwrap(),
    ));
    Ok(Rc::new(CapnProtoTracker::new(entity_manager, bin_writer)))
}

/// This tracker will produce a Perfetto trace file, which unlike the other
/// tracker options can be viewed using the Perfetto UI, rather than
/// gwr-spotter.
#[cfg(feature = "perfetto")]
fn build_perfetto_tracker(
    config: &TrackerConfig,
    monitors: &MonitorsConfig,
) -> Result<Tracker, TrackConfigError> {
    let default_level = if config.filter_regex.is_empty() {
        config.level
    } else {
        log::Level::Error
    };
    let mut entity_manager = EntityManager::new(default_level);
    if !config.filter_regex.is_empty() {
        entity_manager.add_entity_level_filter(config.filter_regex, config.level)?;
    }

    if monitors.enable {
        entity_manager
            .set_monitor_window_size_for(monitors.filter_regex, monitors.window_size_ticks)?;
    }

    let bin_writer: Writer = Box::new(BufWriter::new(
        fs::File::create(config.file.unwrap()).unwrap(),
    ));
    Ok(Rc::new(PerfettoTracker::new(entity_manager, bin_writer)))
}

/// Set up stdout/binary/Perfetto trackers according the the command-line
/// arguments
#[cfg(not(feature = "perfetto"))]
pub fn setup_trackers(config: &TrackersConfig) -> Result<Tracker, TrackConfigError> {
    let multi_tracker_required = config.stdout.enable && config.binary.enable;

    if multi_tracker_required {
        let mut tracker = MultiTracker::default();

        if config.stdout.enable {
            let log_tracker: Tracker = build_stdout_tracker(&config.stdout, &config.monitors)?;
            tracker.add_tracker(log_tracker);
        }
        if config.binary.enable {
            let trace_tracker: Tracker = build_binary_tracker(&config.binary, &config.monitors)?;
            tracker.add_tracker(trace_tracker);
        }

        Ok(Rc::new(tracker))
    } else if config.stdout.enable {
        build_stdout_tracker(&config.stdout, &config.monitors)
    } else if config.binary.enable {
        build_binary_tracker(&config.binary, &config.monitors)
    } else {
        build_stdout_tracker(&TrackerConfig::default(), &MonitorsConfig::default())
    }
}

/// Set up stdout/binary/Perfetto trackers according the the command-line
/// arguments
#[cfg(feature = "perfetto")]
pub fn setup_trackers(config: &TrackersConfig) -> Result<Tracker, TrackConfigError> {
    let multi_tracker_required = [
        config.stdout.enable,
        config.binary.enable,
        config.perfetto.enable,
    ]
    .into_iter()
    .filter(|x| *x)
    .count()
        > 1;

    if multi_tracker_required {
        let mut tracker = MultiTracker::default();

        if config.stdout.enable {
            let log_tracker: Tracker = build_stdout_tracker(&config.stdout, &config.monitors)?;
            tracker.add_tracker(log_tracker);
        }
        if config.binary.enable {
            let trace_tracker: Tracker = build_binary_tracker(&config.binary, &config.monitors)?;
            tracker.add_tracker(trace_tracker);
        }
        if config.perfetto.enable {
            let perfetto_tracker: Tracker =
                build_perfetto_tracker(&config.perfetto, &config.monitors)?;
            tracker.add_tracker(perfetto_tracker);
        }

        Ok(Rc::new(tracker))
    } else if config.stdout.enable {
        build_stdout_tracker(&config.stdout, &config.monitors)
    } else if config.binary.enable {
        build_binary_tracker(&config.binary, &config.monitors)
    } else if config.perfetto.enable {
        build_perfetto_tracker(&config.perfetto, &config.monitors)
    } else {
        build_stdout_tracker(&TrackerConfig::default(), &MonitorsConfig::default())
    }
}
