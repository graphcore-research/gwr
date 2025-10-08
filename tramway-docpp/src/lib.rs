// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A documentation pre-processor
//!
//! Supports a number of helper macros that allow the user to embded content
//! into documentation.

use tramway_doc_builder::{helpers, toc};

#[cfg(feature = "asciidoctor")]
mod adoc;
mod cmd;
mod include_str;
mod section;
#[cfg(feature = "typst")]
mod typst;

/// Embed [asciidoctor](https://docs.asciidoctor.org/asciidoctor/latest/) snippets
///
/// This allows the user to embed [asciidoctor](https://docs.asciidoctor.org/asciidoctor/latest/)
/// snippets where markdown is not able to support the desired formatting.
///
/// It requires `asciidoctor` to be on the path and will use a temporary file to
/// write the snippet to. The `html` output of `aciidoctor` is then captured and
/// emitted verbatim back into the documentation.
///
/// *Note*: `brew install asciidoctor` is the best way to install this on macOS
///
/// # Example
///
/// ```rust
/// #[doc = tramway_docpp::adoc!(
///     "This in an [big]#important# comment"
/// )]
/// fn func() -> String {
///     todo!()
/// }
/// ```
#[cfg(feature = "asciidoctor")]
#[proc_macro]
pub fn adoc(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    adoc::process(input)
}

/// Embed [typst](https://typst.app) snippets
///
/// This allows the user to embed [typst](https://typst.app) snippets where
/// markdown is not able to support the desired formatting.
///
/// It requires `typst` to be on the path and will use a temporary file to
/// write the `typst` snippet to. The output of `typst` is then captured and
/// emitted verbatim back into the documentation in `svg` format.
///
/// # Example
///
/// ```rust
/// #[doc = tramway_docpp::typst!(
///     "$ A = pi r^2 $"
/// )]
/// fn pi_r_squared() -> f64 {
///     todo!()
/// }
/// ```
#[cfg(feature = "typst")]
#[proc_macro]
pub fn typst(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    typst::process(input)
}

/// Embed output of any arbitrary command
///
/// Note that the entire command must be one string, otherwise the token stream
/// inserts spaces between operators and identifiers.
///
/// Will `panic!` if not presented with a command in quotes.
///
/// # Example
///
/// ```rust
/// #[doc = tramway_docpp::cmd!(
///     "echo 'This is my comment for the function'"
/// )]
/// fn function() {}
/// ```
#[proc_macro]
pub fn cmd(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    cmd::process(input)
}

/// Embed the contents of a file.
///
/// Path is relative to the top of the workspace (unlike the builtin
/// `include_str`).
///
/// # Example
///
/// ```rust
/// #[doc = tramway_docpp::include_str!("tramway-docpp/src/lib.rs")]
/// fn function() {
///     todo!()
/// }
/// ```
#[proc_macro]
pub fn include_str(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    include_str::process(input)
}

#[proc_macro]
pub fn section(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    section::process(input)
}

#[proc_macro]
pub fn toc(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    toc::parse(input.into()).into()
}
