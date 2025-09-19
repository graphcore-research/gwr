// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use color_eyre::eyre::Context;
use log::{debug, error};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::Logger;
use crate::command::Command;

pub mod converter;

use crate::recipe::converter::python;

const HEADER: &str = "# Auto-generated file\n";

#[derive(Serialize, Deserialize)]
pub struct Ingredient {
    comment: String,
    command: String,
}

impl Ingredient {
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    #[must_use]
    pub fn comment(&self) -> &str {
        &self.comment
    }
}

#[derive(Serialize, Deserialize)]
pub struct Argument {
    name: String,
    default: Option<String>,
    comment: String,
    value: Option<String>,
}

impl Argument {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn value(&self) -> &Option<String> {
        &self.value
    }

    #[must_use]
    pub fn comment(&self) -> &str {
        &self.comment
    }

    pub fn set_value(&mut self, value: &str) {
        self.value = Some(value.to_string());
    }
}

#[derive(Serialize, Deserialize)]
pub struct Recipe {
    /// Description of what the recipe does. Used when searching for matching
    /// recipes.
    description: String,

    /// List of arguments this recipe takes.
    arguments: Vec<Argument>,

    /// Commands used in this recipe.
    ingredients: Vec<Ingredient>,
}

// Build regular expressions to capture shell-like variables
fn build_regexs() -> (Regex, Regex) {
    //  Capture arguments of the form ${USER}
    let arg_re_bracket = Regex::new(r"\$\{(?<name>[[:word:]]+)\}").unwrap();
    //  Capture arguments of the form $USER
    let arg_re_no_bracket = Regex::new(r"\$(?<name>[[:word:]]+)").unwrap();

    (arg_re_bracket, arg_re_no_bracket)
}

impl Recipe {
    pub fn new_from_file(recipe_path: &Path) -> color_eyre::eyre::Result<Self> {
        let file_contents = fs::read_to_string(recipe_path)
            .wrap_err_with(|| format!("Failed to read {}", recipe_path.display()));
        let mut recipe_result = match file_contents {
            Ok(file_contents) => serde_yaml_ng::from_str::<Recipe>(&file_contents)
                .wrap_err_with(|| format!("Failed to parse contents of {}", recipe_path.display())),
            Err(e) => Err(e),
        };

        if let Ok(recipe) = &mut recipe_result {
            // Copy defaults over to values where needed
            for arg in &mut recipe.arguments {
                if arg.value.is_some() {
                    continue;
                }
                if let Some(value) = &arg.default {
                    arg.value = Some(value.to_string());
                }
            }
        }
        recipe_result
    }

    #[must_use]
    pub fn new(description: &str, commands: &[Command]) -> Self {
        let mut recipe = Recipe {
            description: description.to_string(),
            arguments: Vec::new(),
            ingredients: Vec::new(),
        };

        recipe.build_ingredients_and_args(commands);
        recipe
    }

    /// Given a list of commands, build up the arguments and ingredients from
    /// those that the user has selected
    fn build_ingredients_and_args(&mut self, commands: &[Command]) {
        let (arg_re_bracket, arg_re_no_bracket) = build_regexs();

        // Track unique names of all arguments found in the commands
        let mut arg_names = HashSet::new();

        for command in commands {
            if command.selected() {
                find_arguments(command, &mut arg_names, &arg_re_bracket, &arg_re_no_bracket);
                self.ingredients.push(Ingredient {
                    comment: String::new(),
                    command: command.command().to_string(),
                });
            }
        }

        // Build up the list of arguments for this recipe
        for name in arg_names {
            self.arguments.push(Argument {
                name,
                default: Some(String::new()),
                comment: String::new(),
                value: None,
            });
        }
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub fn arguments(&self) -> &[Argument] {
        &self.arguments
    }

    pub fn set_argument_value(&mut self, arg_index: usize, value: &str) {
        self.arguments[arg_index].set_value(value);
    }

    #[must_use]
    pub fn ingredients(&self) -> &[Ingredient] {
        &self.ingredients
    }

    pub fn print_help(&self) {
        println!("{}:\n", self.description);
        for arg in &self.arguments {
            match &arg.default {
                Some(default) => {
                    println!("  --{}: {} (default = '{default}')", arg.name, arg.comment);
                }
                None => println!("  --{}: {}", arg.name, arg.comment),
            }
        }
    }

    /// Parse arguments from the command-line so that all the current
    pub fn parse_cli_args(&mut self, args: &[String]) {
        self.parse_args(args);
    }

    /// Execute a recipe by writing out a single shell script which is called
    pub fn execute(&mut self, tmp_root: &Path, keep_tmp: bool, logger: &mut impl Logger) {
        let tmp_str = tmp_root.to_string_lossy().to_string() + ".sh";
        let script_path = PathBuf::from(tmp_str);
        if let Err(e) = self.write_script(&script_path) {
            error!("Failed to write {}:\n{e}", script_path.display());
        }

        // Ensure the path is absolute so that there are no issues sourceing it
        let script_path = fs::canonicalize(script_path).unwrap();
        let command = format!("source {}", script_path.display());
        match run_command_as_interactive(&command, logger, true) {
            Err(e) => error!("Running {command} failed to execute:\n{e}"),
            _ => {
                if !keep_tmp {
                    // Ran ok, remove script
                    fs::remove_file(script_path).expect("Should be able to remove tmp file");
                }
            }
        }
    }

    /// Parse the arguments passed by the user and track their values
    ///
    /// Assume using default values if the user hasn't set a value.
    ///
    /// Returns true if successfully parsed, false on error.
    fn parse_args(&mut self, args: &[String]) -> bool {
        let mut args_to_set = HashMap::new();
        let mut arg_name = None;
        for arg in args {
            if arg.starts_with("--") {
                let name: String = arg.chars().skip(2).collect();
                arg_name = Some(name);
            } else if let Some(name) = arg_name.take() {
                args_to_set.insert(name, arg);
            } else {
                error!("Cannot parse '{arg}'");
                self.print_help();
                return false;
            }
        }

        if arg_name.is_some() {
            error!("--{} with no value", arg_name.take().unwrap());
            self.print_help();
            return false;
        }

        for arg in &mut self.arguments {
            match args_to_set.get(&arg.name) {
                Some(value) => arg.value = Some((*value).to_string()),
                None => {
                    if let Some(default) = &arg.default {
                        arg.value = Some(default.to_string());
                    }
                }
            }
            match &arg.value {
                Some(value) => debug!("Setting {} to {}", arg.name, value),
                None => debug!("Not setting {}", arg.name),
            }
        }
        true
    }

    fn write_script(&self, script_path: &PathBuf) -> io::Result<()> {
        debug!("Writing recipe to {}", script_path.display());
        let file = fs::File::create(script_path)?;

        let mut bin_writer = Box::new(BufWriter::new(file));
        bin_writer.write_all(HEADER.as_bytes())?;
        self.write_args_to_script(&mut bin_writer)?;
        self.write_commands_to_script(&mut bin_writer)?;
        Ok(())
    }

    fn write_args_to_script(&self, bin_writer: &mut Box<BufWriter<File>>) -> io::Result<()> {
        for arg in &self.arguments {
            if arg.value.is_none() {
                // Skip undefined arguments - assume they will come from ENV
                continue;
            }

            // Write out the comments if they are defined
            let comment = &arg.comment;
            if !comment.is_empty() {
                bin_writer.write_all(format!("\n# {comment}\n").as_bytes())?;
            }
            let name = &arg.name;
            let value = &arg.value.as_ref().unwrap();
            bin_writer.write_all(format!("export {name}=\"{value}\"\n").as_bytes())?;
        }
        Ok(())
    }

    fn write_commands_to_script(&self, bin_writer: &mut Box<BufWriter<File>>) -> io::Result<()> {
        for command in &self.ingredients {
            // Write out the comments if they are defined
            let comment = &command.comment;
            if !comment.is_empty() {
                bin_writer.write_all(format!("\n# {comment}\n").as_bytes())?;
            }
            let cmd = &command.command;
            bin_writer.write_all(format!("{cmd}\n").as_bytes())?;
        }
        Ok(())
    }

    #[must_use]
    pub fn matches(&self, search_re: &Regex) -> bool {
        if search_re.is_match(&self.description) {
            return true;
        }
        for command in &self.ingredients {
            if search_re.is_match(&command.comment) || search_re.is_match(&command.command) {
                return true;
            }
        }
        false
    }

    pub fn convert_to(&self, out_path: &Path, format: &str) -> io::Result<()> {
        // TODO: Support other formats
        assert_eq!(format, "python");
        python::convert_to(self, out_path)?;
        Ok(())
    }
}

/// Run a command as if it were run in interactive mode by the user.
///
/// zsh:
/// In order to run a command under zsh that would be able to execute any
/// equivalent interactive user commands it needs to be run as:
///    zsh -i --nozle <<< "COMMAND ; setopt zle ; exit"
/// so we need to run and drive stdin.
///
/// bash:
/// Very similar to zsh, but can be simply:
///    bash -i <<< "COMMAND"
///
/// Note that multiple commands can be run by writing a script and passing
/// "source SCRIPT" as the command.
pub fn run_command_as_interactive(
    to_run: &str,
    logger: &mut impl Logger,
    show_output_on_pass: bool,
) -> io::Result<()> {
    debug!("Running '{to_run}'");

    let mut child = if cfg!(target_os = "macos") {
        std::process::Command::new("zsh")
            .env("SHELL_SESSIONS_DISABLE", "1") // Disable saving/restoring zsh sessions
            .arg("-i")
            .arg("--nozle")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Should be able to spawn child process")
    } else {
        std::process::Command::new("bash")
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Should be able to spawn child process")
    };

    let mut stdin = child.stdin.take().expect("Should be able to open stdin");

    let cmd = if cfg!(target_os = "macos") {
        format!("{to_run} ; setopt zle ; exit")
    } else {
        to_run.to_string()
    };

    std::thread::spawn(move || {
        stdin
            .write_all(cmd.as_bytes())
            .expect("Should be able to write to stdin");
    });

    let output = child.wait_with_output()?;
    let success = output.status.success();
    if !success {
        error!("FAILED: '{to_run}'");
        logger.info(str::from_utf8(&output.stdout).unwrap());
        logger.error(str::from_utf8(&output.stderr).unwrap());
    }
    debug!("SUCCESS: '{to_run}'");
    if show_output_on_pass {
        logger.info(str::from_utf8(&output.stdout).unwrap());
        logger.error(str::from_utf8(&output.stderr).unwrap());
    }
    Ok(())
}

/// Parse a command and add the variables found in the command as an argument
fn find_arguments(
    command: &Command,
    arg_names: &mut HashSet<String>,
    arg_re_bracket: &Regex,
    arg_re_no_bracket: &Regex,
) {
    let command_str = command.command();

    // Find all the regular expression matches and keep track of set of argument
    // names
    for cap in arg_re_bracket.captures_iter(command_str) {
        arg_names.insert(cap.name("name").unwrap().as_str().to_string());
    }
    for cap in arg_re_no_bracket.captures_iter(command_str) {
        arg_names.insert(cap.name("name").unwrap().as_str().to_string());
    }
}
