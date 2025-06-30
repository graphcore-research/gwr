// Copyright (c) 2020 Graphcore Ltd. All rights reserved.

use std::env;
use std::path::{Path, PathBuf};

use capnpc::CompilerCommand;
use walkdir::WalkDir;

const CAPNP_BIN_ENV_VAR: &str = "CAPNP_BIN";
const CAPNP_INC_DIR_ENV_VAR: &str = "CAPNP_INC_DIR";

fn get_schemas_root() -> PathBuf {
    PathBuf::from("./schemas")
        .canonicalize()
        .expect("Failed to get absolute path for the root of the schemas")
}

fn watch_tree(root_dir: &Path) {
    for entry in WalkDir::new(root_dir) {
        match entry {
            Ok(entry) => println!("cargo::rerun-if-changed={}", entry.path().display()),
            Err(error) => panic!(
                "Failed to access entry '{}'",
                error.path().unwrap_or_else(|| Path::new("")).display()
            ),
        }
    }
}

fn add_file_build_triggers() {
    let schemas_root = get_schemas_root();
    watch_tree(&schemas_root);
}

fn add_env_build_triggers() {
    println!("cargo::rerun-if-env-changed={CAPNP_BIN_ENV_VAR}");
    println!("cargo::rerun-if-env-changed={CAPNP_INC_DIR_ENV_VAR}");
}

fn get_capnp_schemas() -> Vec<PathBuf> {
    let mut schemas: Vec<PathBuf> = Vec::new();
    for entry in get_schemas_root()
        .read_dir()
        .expect("Failed to read the schema template directory")
    {
        match entry {
            Ok(entry) => {
                let filename = PathBuf::from(entry.file_name());
                if let Some(extension) = filename.extension() {
                    if extension == "capnp" {
                        schemas.push(entry.path());
                    }
                }
            }
            Err(error) => panic!("{}", error),
        }
    }

    schemas
}

fn try_default_capnp_compiler_bin() -> String {
    let default = match std::env::consts::OS {
        "linux" => "/usr/bin/capnp",
        "macos" if std::env::consts::ARCH == "aarch64" => "/opt/homebrew/bin/capnp",
        "macos" if std::env::consts::ARCH == "x86_64" => "/usr/local/homebrew/bin/capnp",
        _ => panic!(
            "The '{CAPNP_BIN_ENV_VAR}' environment variable must be set as the path to the Cap'n Proto 'capnp' executable (no default available for this platform)"
        ),
    };

    PathBuf::from(default).try_exists().unwrap_or_else(|_| panic!(
        "The '{CAPNP_BIN_ENV_VAR}' environment variable must be set as the path to the Cap'n Proto 'capnp' executable (platform default not in use)"
    ));

    default.to_string()
}

fn get_capnp_compiler_bin_env_var() -> PathBuf {
    PathBuf::from(env::var(CAPNP_BIN_ENV_VAR).unwrap_or_else(|_| try_default_capnp_compiler_bin()))
}

fn try_default_capnp_include_dir() -> Option<String> {
    let default = match std::env::consts::OS {
        "linux" => return None,
        "macos" if std::env::consts::ARCH == "aarch64" => "/opt/homebrew/include/capnp/",
        "macos" if std::env::consts::ARCH == "x86_64" => "/usr/local/homebrew/include/capnp/",
        _ => panic!(
            "The '{CAPNP_INC_DIR_ENV_VAR}' environment variable  must be set as the path to the Cap'n Proto include directory (no default available for this platform)"
        ),
    };

    PathBuf::from(default).try_exists().unwrap_or_else(|_| panic!(
        "The '{CAPNP_INC_DIR_ENV_VAR}' environment variable must be set as the path to the Cap'n Proto include directory (platform default not in use)"
    ));

    Some(default.to_string())
}

fn get_capnp_include_dir_env_var() -> Option<PathBuf> {
    let inc_dir = PathBuf::from(
        env::var(CAPNP_INC_DIR_ENV_VAR)
            .unwrap_or_else(|_| try_default_capnp_include_dir().unwrap_or_default()),
    );

    if inc_dir == PathBuf::default() {
        return None;
    }

    Some(inc_dir)
}

fn compile_capnp_schemas(schemas: &[PathBuf]) {
    let mut command = CompilerCommand::new();
    command.capnp_executable(get_capnp_compiler_bin_env_var());
    if let Some(inc_dir) = get_capnp_include_dir_env_var() {
        command.import_path(inc_dir);
    }
    for schema in schemas {
        if let Some(path) = schema.parent() {
            command.src_prefix(path);
        }
        command.file(schema);
    }
    println!("{:#?}", env::var("OUT_DIR"));
    command
        .run()
        .expect("Cap'n Proto compiler failed to generate Rust source");
}

fn main() {
    add_file_build_triggers();
    add_env_build_triggers();

    let schemas = get_capnp_schemas();
    compile_capnp_schemas(&schemas);
}
