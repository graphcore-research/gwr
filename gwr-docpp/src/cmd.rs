// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Process the `cmd!` documentation proc_macro.

use std::process::Command;

use gwr_doc_builder::helpers::{CommandDescriptor, handle_error};
use proc_macro2::Span;
use quote::ToTokens;
use syn::{Error, LitStr};

pub fn process(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let command_descriptor = syn::parse_macro_input!(input as CommandDescriptor);
    handle_error(|| {
        let mut output = String::new();

        for command in command_descriptor.cmd.trim().split(';') {
            let mut args = command.split_whitespace();
            let cmd = args
                .next()
                .ok_or_else(|| Error::new(command_descriptor.span, "no command given"))?;

            let cmd_output = Command::new(cmd).args(args).output().map_err(|_| {
                Error::new(
                    command_descriptor.span,
                    format!("failed to run command `{cmd}`"),
                )
            })?;

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
