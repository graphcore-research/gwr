// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Process the `toc!` documentation proc_macro.

use proc_macro2::Span;
use quote::ToTokens;
use syn::parse::{self, Parse, ParseStream};
use syn::token::{Add, Comma, Eq, Sub};
use syn::{LitStr, bracketed};

use crate::helpers::{env_doc_builder, handle_error, unprocessed};

const INDENT_SPACES: usize = 2;

pub struct TocDocument {
    pub title: String,
    pub sections: Vec<TocNode>,
}

pub struct TocChapter {
    pub title: String,
    pub sections: Vec<TocNode>,
}

pub struct TocSection {
    pub title: String,
    pub path: String,
}

pub enum TocNode {
    Chapter(TocChapter),
    Section(TocSection),
}

impl TocDocument {
    fn to_rust_doc(&self, doc: &mut String, depth: usize) {
        for _ in 0..depth {
            doc.push('#');
        }
        doc.push(' ');
        doc.push_str(self.title.as_str());
        doc.push_str("\n\n");

        for section in &self.sections {
            match section {
                TocNode::Chapter(chapter) => chapter.to_rust_doc(doc, depth + 1),
                TocNode::Section(section) => section.to_rust_doc(doc),
            }
        }
    }

    pub fn dump(&self, depth: usize) {
        let indent = " ".repeat(depth * INDENT_SPACES);
        println!("{}{}:", indent, self.title.as_str());
        for section in &self.sections {
            match section {
                TocNode::Chapter(chapter) => chapter.dump(depth + 1),
                TocNode::Section(section) => section.dump(depth + 1),
            }
        }
    }
}

/// Implementation to parse the token stream and convert it to a [`TocDocument`]
impl Parse for TocDocument {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        input.parse::<Eq>()?;
        let title = input.parse::<LitStr>()?;

        let content;
        bracketed!(content in input);
        let sections = {
            let mut sections = Vec::new();
            while !content.is_empty() {
                sections.push(content.parse()?);
            }
            sections
        };
        Ok(TocDocument {
            title: title.value(),
            sections,
        })
    }
}

impl TocChapter {
    fn to_rust_doc(&self, doc: &mut String, depth: usize) {
        for _ in 0..depth {
            doc.push('#');
        }
        doc.push(' ');
        doc.push_str(self.title.as_str());
        doc.push_str("\n\n");

        for section in &self.sections {
            match section {
                TocNode::Chapter(chapter) => chapter.to_rust_doc(doc, depth + 1),
                TocNode::Section(section) => section.to_rust_doc(doc),
            }
        }
    }

    pub fn dump(&self, depth: usize) {
        let indent = " ".repeat(depth * INDENT_SPACES);
        println!("{}{}:", indent, self.title.as_str());
        for section in &self.sections {
            match section {
                TocNode::Chapter(chapter) => chapter.dump(depth + 1),
                TocNode::Section(section) => section.dump(depth + 1),
            }
        }
    }
}

/// Implementation to parse the token stream and convert it to a [`TocChapter`]
impl Parse for TocChapter {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        input.parse::<Add>()?;
        let title = input.parse::<LitStr>()?;

        let content;
        bracketed!(content in input);
        let sections = {
            let mut sections = Vec::new();
            while !content.is_empty() {
                sections.push(content.parse()?);
            }
            sections
        };
        Ok(TocChapter {
            title: title.value(),
            sections,
        })
    }
}

impl TocSection {
    fn to_rust_doc(&self, doc: &mut String) {
        doc.push('[');
        doc.push_str(self.title.as_str());
        doc.push_str("](");
        doc.push_str(self.path.as_str());
        doc.push_str(")\n\n");
    }

    fn dump(&self, depth: usize) {
        let indent = " ".repeat(depth * INDENT_SPACES);
        println!("{}- {}: {}", indent, self.title, self.path);
    }
}

/// Implementation to parse the token stream and convert it to a [`TocSection`]
impl Parse for TocSection {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        input.parse::<Sub>()?;
        let title = input.parse::<LitStr>()?;
        input.parse::<Comma>()?;
        let path = input.parse::<LitStr>()?;
        Ok(TocSection {
            title: title.value(),
            path: path.value(),
        })
    }
}

/// Implementation to parse the token stream and convert it to a [`TocNode`]
impl Parse for TocNode {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        let lookahead = input.lookahead1();
        if lookahead.peek(Add) {
            let chapter = input.parse::<TocChapter>()?;
            Ok(TocNode::Chapter(chapter))
        } else if lookahead.peek(Sub) {
            let section = input.parse::<TocSection>()?;
            Ok(TocNode::Section(section))
        } else {
            Err(lookahead.error())
        }
    }
}

pub fn parse(input: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    if env_doc_builder() {
        return unprocessed(input, "toc");
    }

    let chapter = syn::parse2::<TocDocument>(input).unwrap();
    handle_error(|| {
        let mut output = "".to_owned();
        chapter.to_rust_doc(&mut output, 1);

        Ok(LitStr::new(output.as_str(), Span::call_site()).into_token_stream())
    })
}
