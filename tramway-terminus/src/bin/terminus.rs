// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! # TRAMWAY Command-Line Assistant
//!
//! `terminus` is a command-line utility to the creation of recipes (sequences
//! of commands). It supports the writing (capture) of recipes as well as the
//! running (execution) and conversion to other formats like Python scripts.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::{fs, io};

use clap::{Parser, Subcommand};
use color_eyre::Result;
use crossterm::event::{self, Event};
use log::{LevelFilter, info};
use ratatui::Terminal;
use ratatui::prelude::CrosstermBackend;
use tramway_terminus::CliLogger;
use tramway_terminus::recipe::{Recipe, run_command_as_interactive};
use tramway_terminus::tui::Tui;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Tool for creating and running recipes (sequences of commands)")]
struct Cli {
    /// Enable debug log messages
    #[arg(short, long)]
    debug: bool,

    /// Recipes folder to search for recipe
    #[arg(short, long, default_value = "terminus/recipes")]
    recipes_folder: String,

    /// Root of tmp file names
    #[arg(long, default_value = ".tmp")]
    tmp_file_root: PathBuf,

    /// Leave any temporary files in place in order to aid debug
    #[arg(long)]
    keep_tmp: bool,

    #[clap(subcommand)]
    command: CommandArg,
}

#[derive(Debug, Subcommand)]
enum CommandArg {
    /// Write a new recipe. Just pass the history file to launch the TUI.
    /// If you pass the recipe name it will write it automatically.
    Write {
        /// History file containing commands to build recipe.
        #[arg(long)]
        history: Option<PathBuf>,

        /// Name of recipe to write.
        #[arg(short, long)]
        recipe: Option<String>,

        #[expect(rustdoc::invalid_html_tags)]
        /// Lines from history file to use. Use [<MODE>[RANGE|INDEX]+;]+
        /// Where:
        ///   MODE: is one of 's'elect, 'd'eselect, 't'oggle.
        #[arg(short, long, verbatim_doc_comment)]
        select: Option<String>,

        /// Recipe description.
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Run a recipe
    Run {
        /// Name of recipe file to run.
        #[arg(short, long)]
        recipe: Option<String>,

        /// Print help for the recipe and its arguments.
        #[arg(long)]
        recipe_help: bool,

        /// Arguments to the recipe.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        args: Vec<String>,
    },
    /// Convert a recipe to another format
    Convert {
        /// Name of recipe file to convert.
        #[arg(short, long)]
        recipe: String,

        /// Output file name
        #[arg(long)]
        out: String,

        /// What output format to write
        #[arg(long, default_value = "python", value_parser = clap::builder::PossibleValuesParser::new(["python", "bash"]))]
        format: String,
    },
}

/// Configure the logger level and formating string.
fn setup_logger(debug: bool) {
    let level = if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    env_logger::builder()
        .filter_level(level)
        .format(|buf, record| writeln!(buf, "{}: {}", record.level(), record.args()))
        .init();
}

/// Build the history
fn capture_history_to_tmp(tmp_root: &Path, n: u32) -> PathBuf {
    let tmp_str = tmp_root.to_string_lossy().to_string() + ".hist";
    let tmp_path = PathBuf::from(tmp_str);
    let command = format!("fc -l -{n} > {}", tmp_path.display());
    let mut cli_logger = CliLogger {};
    run_command_as_interactive(&command, &mut cli_logger, false)
        .expect("Should be able to create history");
    tmp_path
}

fn main() -> Result<()> {
    let args = Cli::parse();

    setup_logger(args.debug);

    let recipes_folder = &args.recipes_folder;
    let tmp_root = args.tmp_file_root.as_path();
    let keep_tmp = args.keep_tmp;

    match &args.command {
        CommandArg::Write {
            history,
            recipe,
            select,
            description,
        } => {
            let history_path = match history {
                Some(history) => history,
                None => &capture_history_to_tmp(tmp_root, 100),
            };
            let result = if let Some(recipe) = recipe {
                write_recipe(history_path, recipe, select, description)
            } else {
                start_write_recipe_tui(history_path, recipes_folder)
            };

            if !keep_tmp && history.is_none() {
                // When using a temporary file, clean it up
                fs::remove_file(history_path.as_path())
                    .expect("Should be able to cleanup history tmp");
            }

            result
        }
        CommandArg::Run {
            recipe,
            recipe_help,
            args,
        } => {
            if let Some(recipe) = recipe {
                run_recipe(tmp_root, keep_tmp, recipe, *recipe_help, args)
            } else {
                start_run_recipe_tui(recipes_folder, tmp_root, keep_tmp)
            }
        }
        CommandArg::Convert {
            recipe,
            out,
            format,
        } => {
            let recipe_path = PathBuf::from(recipe);
            let recipe = Recipe::new_from_file(recipe_path.as_path())?;

            let out_path = PathBuf::from(out);
            Ok(recipe.convert_to(out_path.as_path(), format.as_str())?)
        }
    }
}

// Need to allow this because the code is generated by Clap
fn run_recipe(
    tmp_root: &Path,
    keep_tmp: bool,
    recipe: &String,
    recipe_help: bool,
    args: &[String],
) -> color_eyre::eyre::Result<()> {
    let recipe_path = PathBuf::from(recipe);
    let mut recipe = Recipe::new_from_file(recipe_path.as_path())?;
    if recipe_help {
        recipe.print_help();
    } else {
        recipe.parse_cli_args(args);
        recipe.execute(tmp_root, keep_tmp, &mut CliLogger {});
    }
    color_eyre::eyre::Ok(())
}

// Need to allow this because the code is generated by Clap
#[expect(clippy::ref_option)]
fn write_recipe(
    history_path: &PathBuf,
    recipe: &str,
    select: &Option<String>,
    description: &Option<String>,
) -> Result<()> {
    // Don't need a recipes folder given we will set the full filename from the
    // command-line
    let mut app = tramway_terminus::writer::app::App::new(history_path, "", false);
    if let Some(select) = select {
        app.select_command().set_value(select);
        info!("Selecting {select}");
        app.process_select_command();
    }

    app.recipe_filename().set_value(recipe);
    if let Some(description) = description {
        app.recipe_description().set_value(description);
    }
    info!("Writing {recipe}");
    app.write_recipe();

    color_eyre::eyre::Ok(())
}

fn start_write_recipe_tui(history: &PathBuf, recipes_folder: &str) -> Result<()> {
    color_eyre::install()?;
    let mut app = tramway_terminus::writer::app::App::new(history, recipes_folder, true);

    // Initialize the terminal user interface.
    let backend = CrosstermBackend::new(io::stderr());
    let terminal = Terminal::new(backend)?;
    let mut tui = Tui::new(terminal);
    tui.init()?;

    // Start the main loop.
    while app.is_running() {
        // Render the user interface.
        tui.draw(&mut app)?;
        // Handle events.
        if let Event::Key(key_event) = event::read()? {
            tramway_terminus::writer::handler::handle_key_event(key_event, &mut app);
        }
    }

    // Exit the user interface.
    tui.exit()?;
    color_eyre::eyre::Ok(())
}

fn start_run_recipe_tui(recipes_folder: &str, tmp_root: &Path, keep_tmp: bool) -> Result<()> {
    color_eyre::install()?;
    let mut app = tramway_terminus::runner::app::App::new(recipes_folder, tmp_root, keep_tmp);

    // Initialize the terminal user interface.
    let backend = CrosstermBackend::new(io::stderr());
    let terminal = Terminal::new(backend)?;
    let mut tui = Tui::new(terminal);
    tui.init()?;

    // Start the main loop.
    while app.is_running() {
        // Render the user interface.
        tui.draw(&mut app)?;
        // Handle events.
        if let Event::Key(key_event) = event::read()? {
            tramway_terminus::runner::handler::handle_key_event(key_event, &mut app);
        }
    }

    // Exit the user interface.
    tui.exit()?;
    color_eyre::eyre::Ok(())
}
