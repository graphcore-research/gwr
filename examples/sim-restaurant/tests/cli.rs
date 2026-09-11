// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::process::Command;

use clap::Parser;
use figment::Figment;
use figment::providers::{Format, Toml};
use sim_restaurant::config::RestaurantArgsPartial;
use sim_restaurant::time_of_day::TimeOfDay;

#[test]
fn both_binaries_advertise_copyable_time_defaults() {
    for binary in [
        env!("CARGO_BIN_EXE_sim-restaurant"),
        env!("CARGO_BIN_EXE_sim-restaurant-tui"),
    ] {
        let output = Command::new(binary).arg("--help").output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let help = String::from_utf8(output.stdout).unwrap();
        for (flag, expected) in [("--opening-time", "07:00"), ("--closing-time", "22:00")] {
            let option = help.split_once(flag).unwrap().1.split("--").next().unwrap();
            let value = option
                .split_once("[default: ")
                .unwrap()
                .1
                .split_once(']')
                .unwrap()
                .0;
            assert_eq!(value, expected, "{binary}: {flag}");
            let parsed = RestaurantArgsPartial::try_parse_from(["test", flag, value]).unwrap();
            let time = value.parse::<TimeOfDay>().unwrap();
            if flag == "--opening-time" {
                assert_eq!(parsed.opening_time, Some(time));
            } else {
                assert_eq!(parsed.closing_time, Some(time));
            }
        }
    }
}

#[test]
fn debug_time_defaults_round_trip_through_toml() {
    let opening = TimeOfDay::from_hm(7, 0);
    let closing = TimeOfDay::from_hm(22, 0);
    let toml = format!("opening_time = '{opening:?}'\nclosing_time = '{closing:?}'");
    let parsed: RestaurantArgsPartial = Figment::from(Toml::string(&toml)).extract().unwrap();
    assert_eq!(parsed.opening_time, Some(opening));
    assert_eq!(parsed.closing_time, Some(closing));
}
