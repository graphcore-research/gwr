// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#![allow(dead_code, unused_imports)]
#![doc = gwr_docpp::toc!(
    = "GWR Documentation"
    [
        - "Overview", "self"
        + "API"
        [
            - "Widget", "crate::Widget"
            - "Widget again", "crate::Widget"
            + "Modules"
            [
                + "Nested"
                [
                    - "Nested module", "self::nested"
                ]
            ]
            - "Missing", "crate::missing"
        ]
    ]
)]
#![doc = " Crate documentation."]
#![cfg_attr(
    feature = "asciidoctor",
    doc = gwr_docpp::adoc!(
        " Crate overview with [`Widget`](crate::Widget) and [`nested`](crate::nested)."
    )
)]
#![doc = " = Crate details"]
#![doc(test(attr(warn(unused))))]

extern crate core as ignored_core;

use core::fmt::Debug as IgnoredUse;

const IGNORED_CONST: usize = 0;
static IGNORED_STATIC: usize = 0;
type IgnoredType = usize;

enum IgnoredEnum {}

trait IgnoredTrait {}

union IgnoredUnion {
    value: usize,
}

unsafe extern "C" {
    fn ignored_foreign_function();
}

#[doc = " Widget documentation."]
#[doc = " = Widget details"]
pub struct Widget {
    #[doc = " Field documentation."]
    pub field: usize,
}

impl Widget {
    pub fn method(&self) {}
}

pub struct TupleWidget(usize);
pub struct UnitWidget;

#[doc = " Function documentation."]
pub fn documented_function() {}

#[doc = gwr_docpp::cmd!("printf Command-documentation.")]
pub fn command_documentation() {}

#[doc = gwr_docpp::section!(title = "Generated section", text = "Section body")]
pub fn generated_section() {}

#[doc = gwr_docpp::include_str!("gwr-docpp/tests/fixtures/included.txt")]
pub fn included_documentation() {}

#[cfg_attr(
    feature = "typst",
    doc = gwr_docpp::typst!(
        r#"See #link("https://example.com/api")[the API reference]."#
    )
)]
pub fn typst_documentation() {}

#[doc = " Nested module documentation."]
#[doc = " = Nested details"]
pub mod nested {
    #[doc = " Nested function documentation."]
    pub fn nested_function() {}
}

fn main() {}
