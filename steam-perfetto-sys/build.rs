// Copyright (c) 2024 Graphcore Ltd. All rights reserved.

//! All Perfetto's dependencies will be downloaded and built, and a symlink
//! (see the PERFETTO_SYMLINK const) created in the source tree of this package
//! to give users of this package constant paths to tools and schema.
//!
//! Due to the Perfetto build system performing an in tree build we deliberately
//! avoid letting Cargo watch the perfetto/out directory to ensure that
//! incremental Cargo builds remain fast.
//!
//! Currently just enough is installed and built to support the use of the
//! Perfetto UI.

use std::process::exit;

const STEAM_DOCS_ONLY_ENV_VAR: &str = "STEAM_DOCS_ONLY";
const DOCS_RS_ENV_VAR: &str = "DOCS_RS";

#[cfg(feature = "_perfetto_src")]
mod perfetto_consts {
    pub const PERFETTO_REPO_URL: &str = "https://github.com/google/perfetto";
    pub const PERFETTO_REPO_REFSPEC: &str = "v50.1";

    pub const PERFETTO_SYMLINK: &str = "./perfetto";
}

fn add_file_build_triggers() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/lib.rs");
}

fn add_env_build_triggers() {
    println!("cargo::rerun-if-env-changed={STEAM_DOCS_ONLY_ENV_VAR}");
    println!("cargo::rerun-if-env-changed={DOCS_RS_ENV_VAR}");
}

fn main() {
    add_file_build_triggers();
    add_env_build_triggers();

    if std::env::var(STEAM_DOCS_ONLY_ENV_VAR).is_ok() || std::env::var(DOCS_RS_ENV_VAR).is_ok() {
        // Do not attempt to download or build Perfetto if a documentation only
        // build is occuring, regardless of the features enabled. This is useful
        // as it allows the `--all-features` flag to be passed to rustdoc
        // without incurring the time penalty of the Perfetto build.
        //
        // `DOCS_RS` is also supported as it is the defacto-standard mechanism
        // used, due to a lack of support for detecting documentation builds in
        // Cargo, see https://docs.rs/about/builds#detecting-docsrs.
        // However as the build of the Rocket crate fails when `DOCS_RS` is set,
        // within the STEAM infrastructure only `STEAM_DOCS_ONLY` is used.
        exit(0);
    }

    #[cfg(feature = "_perfetto_src")]
    {
        use std::env;
        use std::process::Command;

        let out_dir = env::var("OUT_DIR").unwrap();

        Command::new("git")
            .arg("init")
            .arg(&out_dir)
            .output()
            .expect("Failed to init repo for Perfetto source");

        Command::new("git")
            .args(["-C", &out_dir])
            .arg("fetch")
            .args(["--depth", "1"])
            .arg(perfetto_consts::PERFETTO_REPO_URL)
            .arg(perfetto_consts::PERFETTO_REPO_REFSPEC)
            .output()
            .expect("Failed to fetch Perfetto source repo");

        Command::new("git")
            .args(["-C", &out_dir])
            .arg("checkout")
            .arg("FETCH_HEAD")
            .output()
            .expect("Failed to checkout Perfetto source repo");

        Command::new("ln")
            .arg("-s")
            .arg("-F")
            .arg("-h")
            .arg(&out_dir)
            .arg(perfetto_consts::PERFETTO_SYMLINK)
            .output()
            .expect("Failed to create symlink to Perfetto source repo");

        #[cfg(feature = "_perfetto_ui")]
        {
            Command::new(format!("{}/tools/install-build-deps", &out_dir))
                .arg("--ui")
                .output()
                .expect("Failed to install build dependencies");
            println!("cargo::rerun-if-changed=perfetto/buildtools");
            println!("cargo::rerun-if-changed=perfetto/third_party");

            Command::new(format!("{}/ui/build", &out_dir))
                .output()
                .expect("Failed to build Perfetto UI");
        }
    }
}
