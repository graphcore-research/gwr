// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Process the `cmd!` documentation proc_macro.

use std::process::Command;

use proc_macro2::Span;
use quote::ToTokens;
use steam_doc_builder::helpers::{CommandDescriptor, handle_error};
use syn::LitStr;

pub fn process(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let command_descriptor = syn::parse_macro_input!(input as CommandDescriptor);
    handle_error(|| {
        let mut output = String::new();

        let commands = command_descriptor.cmd.trim().split(';');
        for command in commands {
            let args: Vec<_> = command.trim().split(' ').collect();

            let cmd = args.first().expect("No command given");

            let cmd_output = Command::new(cmd)
                .args(args.into_iter().skip(1))
                .output()
                .expect("\n\n**failed to run command**");

            output.push_str(
                String::from_utf8_lossy(&cmd_output.stdout)
                    .to_string()
                    .as_str(),
            );
            output.push_str(
                String::from_utf8_lossy(&cmd_output.stderr)
                    .to_string()
                    .as_str(),
            );
        }

        Ok(LitStr::new(&output, Span::call_site()).into_token_stream())
    })
    .into()
}
