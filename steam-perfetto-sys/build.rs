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

    #[cfg(feature = "_perfetto_src")]
    {
        use std::env;
        use std::process::Command;

        let out_dir = env::var("OUT_DIR").unwrap();

        let output = Command::new("git")
            .arg("init")
            .arg(&out_dir)
            .output()
            .expect("git command failed to start");
        assert!(
            output.status.success(),
            "Failed to init repo for Perfetto source:\n{}",
            str::from_utf8(&output.stderr).unwrap_or_default()
        );

        let output = Command::new("git")
            .args(["-C", &out_dir])
            .arg("fetch")
            .args(["--depth", "1"])
            .arg(perfetto_consts::PERFETTO_REPO_URL)
            .arg(perfetto_consts::PERFETTO_REPO_REFSPEC)
            .output()
            .expect("git command failed to start");
        assert!(
            output.status.success(),
            "Failed to fetch Perfetto source repo:\n{}",
            str::from_utf8(&output.stderr).unwrap_or_default()
        );

        let output = Command::new("git")
            .args(["-C", &out_dir])
            .arg("checkout")
            .arg("FETCH_HEAD")
            .output()
            .expect("git command failed to start");
        assert!(
            output.status.success(),
            "Failed to checkout Perfetto source repo:\n{}",
            str::from_utf8(&output.stderr).unwrap_or_default()
        );

        let output = Command::new("ln")
            .arg("-s")
            .arg("-F")
            .arg("-n")
            .arg(&out_dir)
            .arg(perfetto_consts::PERFETTO_SYMLINK)
            .output()
            .expect("ln command failed to start");
        assert!(
            output.status.success(),
            "Failed to create symlink to Perfetto source repo:\n{}",
            str::from_utf8(&output.stderr).unwrap_or_default()
        );

        if std::env::var(STEAM_DOCS_ONLY_ENV_VAR).is_err()
            && std::env::var(DOCS_RS_ENV_VAR).is_err()
        {
            // Only allow the build of Perfetto when a regular build is
            // occuring. Perfetto will not be built during documentation-only
            // builds, regardless of the features enabled.
            //
            // This is useful as it allows the `--all-features` flag to be
            // passed to rustdoc without incurring the time penalty of the
            // Perfetto build.
            //
            // `DOCS_RS` is also supported as it is the defacto-standard
            // mechanism used, due to a lack of support for detecting
            // documentation builds in Cargo, see
            // https://docs.rs/about/builds#detecting-docsrs.
            // However as the build of the Rocket crate fails when `DOCS_RS` is
            // set, within the STEAM infrastructure only `STEAM_DOCS_ONLY` is
            // used.

            #[cfg(feature = "_perfetto_ui")]
            {
                let output = Command::new(format!("{}/tools/install-build-deps", &out_dir))
                    .arg("--ui")
                    .output()
                    .expect("install-build-deps command failed to start");
                assert!(
                    output.status.success(),
                    "Failed to install build dependencies:\n{}",
                    str::from_utf8(&output.stderr).unwrap_or_default()
                );

                println!("cargo::rerun-if-changed=perfetto/buildtools");
                println!("cargo::rerun-if-changed=perfetto/third_party");

                let output = Command::new(format!("{}/ui/build", &out_dir))
                    .output()
                    .expect("build command failed to start");
                assert!(
                    output.status.success(),
                    "Failed to build Perfetto UI:\n{}",
                    str::from_utf8(&output.stderr).unwrap_or_default()
                );
            }
        }
    }
}
