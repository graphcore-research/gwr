// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A library for document parsing.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::process::exit;
use std::rc::Rc;

use regex::Regex;
use syn::parse::{self, Parse, ParseStream};
use syn::{Attribute, Item};

use crate::asciidoctor_pp::AsciiDoctorPreProcessor;
use crate::doc_nodes::{
    DocNode, DocNodeCommon, Field, Function, Module, Struct, write_section_header,
};

macro_rules! trace {
    ($obj:ident, $fmt:expr) => {
        if $obj.verbose {
            println!($fmt)
        }
    };
    ($obj:ident, $fmt:expr, $($tokens:tt)*) => {
        if $obj.verbose {
            println!($fmt, $($tokens)*)
        }
    };
}

/// A document parser used to produce asciidoctor documentation.
pub struct DocParser {
    /// Enables verbose logging.
    verbose: bool,

    /// Regex used to extract `toc` sections from documentation snippets.
    toc_regex: Regex,

    /// Regex used to determine if `self` is in a path name
    self_regex: Regex,

    /// Registery of all modules parsed in the parsed document.
    doc_elements: HashMap<String, Rc<RefCell<DocNodeCommon>>>,

    /// Pre-processor used to create AsciiDoctor output
    adoc_pp: AsciiDoctorPreProcessor,
}

impl DocParser {
    /// Create a document parser.
    #[must_use]
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            toc_regex: Regex::new(r"^(?<before>.*)#\[doc = toc\((?<toc>[^\)]*)\)(?<after>.*)$")
                .unwrap(),
            self_regex: Regex::new(r"^self(?<after>::.*)?$").unwrap(),
            doc_elements: HashMap::new(),
            adoc_pp: AsciiDoctorPreProcessor::new(),
        }
    }

    /// Parse the specified Rust file (usually a file produced with `cargo
    /// expand`).
    ///
    /// Returns the top-level module in the file which should be the crate.
    pub fn parse_doc(&mut self, input_file: &str) -> Rc<RefCell<DocNodeCommon>> {
        let mut file =
            File::open(input_file).unwrap_or_else(|_| panic!("Failed to open {input_file}"));

        let mut content = String::new();
        file.read_to_string(&mut content)
            .unwrap_or_else(|_| panic!("Failed to read {input_file} contents"));

        let ast = syn::parse_file(&content)
            .unwrap_or_else(|_| panic!("Failed to parse contents of {input_file}"));

        let top_path = "crate".to_owned();
        let m = Module::new();
        let mut top = DocNodeCommon::new_rc(top_path, None, DocNode::Module(m));
        self.add_doc_node(top.clone());
        self.process_attributes(&mut top, &ast.attrs);
        self.process_items(&mut top, &ast.items);
        top
    }

    fn add_doc_node(&mut self, element: Rc<RefCell<DocNodeCommon>>) {
        let path = element.borrow().full_name();
        trace!(self, "Adding {} ({})", path, element.borrow().node_type());
        let existing = self.doc_elements.insert(path.to_string(), element);
        if let Some(existing) = existing {
            let existing = existing.borrow();
            panic!(
                "{} {} added twice",
                existing.node_type(),
                existing.full_name()
            );
        }
    }

    /// Write out the asciidoctor for the specified module.
    ///
    /// Given a path (in the form of crate::path::to::module) write out the
    /// document as defined by the `toc` document attribute (parsed by
    /// [`toc`](crate::toc) reader).
    pub fn write_adoc(&self, out: &mut File, path_to_toc: &str) {
        let doc_node = self.doc_elements.get(path_to_toc);
        if doc_node.is_none() {
            eprintln!("ERROR: {path_to_toc} not found");
            exit(1);
        }

        let doc_node = doc_node.unwrap().borrow();
        let mut emitted = HashSet::new();
        let depth = 1;

        if let Some(document) = &doc_node.toc {
            emitted.insert(path_to_toc.to_owned());
            write_section_header(out, &document.title, depth, None);
            doc_node.write(self, out, &mut emitted, depth + 1);
        } else {
            eprintln!("ERROR: {path_to_toc} does not contain TOC");
            exit(1);
        }
    }

    pub fn write_doc_node(
        &self,
        out: &mut File,
        path: &str,
        emitted: &mut HashSet<String>,
        depth: usize,
        parent: &DocNodeCommon,
    ) {
        let processed_path = self.process_path(path, parent);
        match self.doc_elements.get(processed_path.as_str()) {
            Some(module) => {
                if emitted.contains(&processed_path.to_string()) {
                    if path == "self" {
                        // This is where the documentation should be emitted
                        module.borrow().write_doc(self, out, depth);
                    } else {
                        // Reference to this section - just create a link
                        out.write_all(format!("See <<{processed_path}>>\n\n").as_bytes())
                            .expect("Failed to write");
                    }
                } else {
                    emitted.insert(processed_path);
                    module.borrow().write(self, out, emitted, depth);
                }
            }
            None => eprintln!("WARNING: module {processed_path} not found"),
        }
    }

    pub fn extract_toc(&mut self, content: String) -> (String, String) {
        if let Some(e) = self.toc_regex.captures(content.as_str()) {
            let before = e.name("before").unwrap().as_str();
            let toc = e.name("toc").unwrap().as_str();
            let after = e.name("after").unwrap().as_str();
            (toc.to_owned(), format!("{before}{after}"))
        } else {
            (String::new(), content)
        }
    }

    #[must_use]
    pub fn process_path(&self, path: &str, parent: &DocNodeCommon) -> String {
        if let Some(e) = self.self_regex.captures(path) {
            let parent_path = parent.full_name();
            match e.name("after") {
                Some(after) => {
                    format!("{parent_path}{}", after.as_str())
                }
                None => parent_path,
            }
        } else {
            path.to_owned()
        }
    }

    fn process_items(&mut self, parent: &mut Rc<RefCell<DocNodeCommon>>, items: &Vec<Item>) {
        for item in items {
            self.process_item(parent, item);
        }
    }

    fn process_item(&mut self, parent: &mut Rc<RefCell<DocNodeCommon>>, item: &Item) {
        match item {
            Item::Const(_) => {
                trace!(self, "Const");
            }
            Item::Enum(_) => {
                trace!(self, "Enum");
            }
            Item::ExternCrate(_) => {
                trace!(self, "ExternCrate");
            }
            Item::Fn(item_fn) => {
                let name = item_fn.sig.ident.to_string();
                trace!(self, "Fn: {}", name);
                let f = DocNode::Function(Function::new());
                let mut obj = DocNodeCommon::new_rc(name, Some(parent.clone()), f);
                self.process_attributes(&mut obj, &item_fn.attrs);
                parent.borrow_mut().add_function(obj);
            }
            Item::ForeignMod(_) => {
                trace!(self, "ForeignMod");
            }
            Item::Impl(_) => {
                trace!(self, "Impl");
            }
            Item::Macro(_) => {
                trace!(self, "Macro");
            }
            Item::Macro2(_) => {
                trace!(self, "Macro2");
            }
            Item::Mod(item_mod) => {
                trace!(self, "Mod: {}", item_mod.ident);
                let name = item_mod.ident.to_string();
                let m = DocNode::Module(Module::new());
                let mut obj = DocNodeCommon::new_rc(name, Some(parent.clone()), m);
                self.add_doc_node(obj.clone());
                self.process_attributes(&mut obj, &item_mod.attrs);
                if let Some(content) = &item_mod.content {
                    self.process_items(&mut obj, &content.1);
                }
                parent.borrow_mut().add_submodule(obj);
            }
            Item::Static(_) => {
                trace!(self, "Static");
            }
            Item::Struct(item_struct) => {
                let name = item_struct.ident.to_string();
                trace!(self, "Struct: {}", name);
                let s = DocNode::Struct(Struct::new());
                let mut obj = DocNodeCommon::new_rc(name, Some(parent.clone()), s);
                self.add_doc_node(obj.clone());

                self.process_attributes(&mut obj, &item_struct.attrs);
                self.process_fields(&mut obj, &item_struct.fields);
                parent.borrow_mut().add_struct(obj);
            }
            Item::Trait(_) => {
                trace!(self, "Trait");
            }
            Item::TraitAlias(_) => {
                trace!(self, "TraitAlias");
            }
            Item::Type(_) => {
                trace!(self, "Type");
            }
            Item::Union(_) => {
                trace!(self, "Union");
            }
            Item::Use(_) => {
                trace!(self, "Use");
            }
            Item::Verbatim(_) => {
                trace!(self, "Verbatim");
            }
            _ => {
                trace!(self, "other");
            }
        }
    }

    fn process_attributes(&mut self, obj: &mut Rc<RefCell<DocNodeCommon>>, attrs: &Vec<Attribute>) {
        for attr in attrs {
            self.process_attribute(obj, attr);
        }
    }

    fn process_attribute(&mut self, obj: &mut Rc<RefCell<DocNodeCommon>>, attr: &Attribute) {
        if attr.path.is_ident("doc") {
            match syn::parse2::<CommentDescriptor>(attr.tokens.clone()) {
                Ok(descriptor) => {
                    let str = descriptor.comment.to_owned();
                    obj.borrow_mut().push_doc_string(self, str.as_str());
                }
                Err(_) => {
                    // Probably a different doc directive like
                    //  #![doc(test(attr(warn(unused))))]
                }
            }
        }
    }

    fn process_fields(&mut self, obj: &mut Rc<RefCell<DocNodeCommon>>, fields: &syn::Fields) {
        match fields {
            syn::Fields::Named(named) => {
                for field in &named.named {
                    let name = field.ident.as_ref().unwrap().to_string();
                    let f = DocNode::Field(Field::new());
                    let mut f = DocNodeCommon::new_rc(name, Some(obj.clone()), f);
                    self.process_attributes(&mut f, &field.attrs);
                    obj.borrow_mut().add_field(f);
                }
            }
            _ => {
                // Ignore Unnamed / Unit as they won't have extra docs
            }
        }
    }

    #[must_use]
    pub fn preprocess_doc(&self, input: &str, depth: usize) -> String {
        self.adoc_pp.preprocess_doc(input, depth)
    }
}

/// Represents the TokenStream for comment lines.
///
/// They are of the form #[doc = "Comment"] where the `doc` is pulled off
/// and the `= "Comment"` is the TokenStream
struct CommentDescriptor {
    comment: String,
}

/// Parse comment token streams to build [`CommentDescriptor`]
impl Parse for CommentDescriptor {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        let _eq = input.parse::<syn::token::Eq>()?;
        let comment = input.parse::<syn::LitStr>()?;
        Ok(CommentDescriptor {
            comment: comment.value(),
        })
    }
}
