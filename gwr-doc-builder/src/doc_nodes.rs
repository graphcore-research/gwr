// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A library for document parsing.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::rc::Rc;
use std::str::FromStr;

use proc_macro2::TokenStream;

use crate::doc_parser::DocParser;
use crate::toc::{TocChapter, TocDocument, TocNode, TocSection};

const PATH_SEP: &str = "::";

pub fn write_section_header(out: &mut File, title: &str, depth: usize, reference: Option<&str>) {
    if let Some(reference) = reference {
        out.write_all(format!("[#{reference}]\n").as_bytes())
            .expect("Failed to write");
    }
    out.write_all(format!("{} {}\n\n", "=".repeat(depth), title).as_bytes())
        .expect("Failed to write");
}

/// Structure which manage the differences between documentation nodes.
pub enum DocNode {
    Module(Module),
    Function(Function),
    Struct(Struct),
    Field(Field),
}

/// Structure to capture the common functionality of all bits of the document.
pub struct DocNodeCommon {
    name: String,
    parent: Option<Rc<RefCell<DocNodeCommon>>>,
    doc: String,
    pub toc: Option<TocDocument>,
    node: DocNode,
}

impl DocNodeCommon {
    #[must_use]
    pub fn new(name: String, parent: Option<Rc<RefCell<DocNodeCommon>>>, node: DocNode) -> Self {
        Self {
            name,
            doc: String::new(),
            toc: None,
            node,
            parent,
        }
    }

    /// Create a new [`DocNodeCommon`] and wrap it in an Rc<RefCell<>>.
    #[must_use]
    pub fn new_rc(
        name: String,
        parent: Option<Rc<RefCell<DocNodeCommon>>>,
        node: DocNode,
    ) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(DocNodeCommon::new(name, parent, node)))
    }

    #[must_use]
    pub fn node_type(&self) -> &'static str {
        match self.node {
            DocNode::Module(_) => "Module",
            DocNode::Struct(_) => "Struct",
            DocNode::Function(_) => "Function",
            DocNode::Field(_) => "Field",
        }
    }

    pub fn write(
        &self,
        parser: &DocParser,
        out: &mut File,
        emitted: &mut HashSet<String>,
        depth: usize,
    ) {
        if let Some(d) = &self.toc {
            // Normally skip the document header
            for node in &d.sections {
                match node {
                    TocNode::Chapter(c) => {
                        write_section_header(out, &c.title, depth, None);
                        self.write_chapter(parser, out, c, emitted, depth + 1);
                    }
                    TocNode::Section(s) => self.write_section(parser, out, s, emitted, depth),
                }
            }
        } else {
            self.write_doc(parser, out, depth);
        }
    }

    pub fn write_doc(&self, parser: &DocParser, out: &mut File, depth: usize) {
        let doc = parser.preprocess_doc(self.doc.as_str(), depth);
        out.write_all(format!("{doc}\n\n").as_bytes())
            .expect("Failed to write");
    }

    fn write_chapter(
        &self,
        parser: &DocParser,
        out: &mut File,
        chapter: &TocChapter,
        emitted: &mut HashSet<String>,
        depth: usize,
    ) {
        for node in &chapter.sections {
            match node {
                TocNode::Chapter(c) => {
                    write_section_header(out, &c.title, depth, None);
                    self.write_chapter(parser, out, c, emitted, depth + 1);
                }
                TocNode::Section(s) => self.write_section(parser, out, s, emitted, depth),
            }
        }
    }

    fn write_section(
        &self,
        parser: &DocParser,
        out: &mut File,
        section: &TocSection,
        emitted: &mut HashSet<String>,
        depth: usize,
    ) {
        let header = parser.process_path(section.path.as_str(), self);
        if emitted.contains(&header) {
            write_section_header(out, &section.title, depth, None);
        } else {
            write_section_header(out, &section.title, depth, Some(header.as_str()));
        }
        parser.write_doc_node(out, section.path.as_str(), emitted, depth + 1, self);
    }

    #[must_use]
    pub fn full_name(&self) -> String {
        let mut path = Vec::new();
        self.full_path(&mut path);
        path.join(PATH_SEP)
    }

    fn full_path(&self, path: &mut Vec<String>) {
        if let Some(parent) = &self.parent {
            parent.borrow().full_path(path);
        }
        path.push(self.name.to_string());
    }

    pub fn push_doc_string(&mut self, parser: &mut DocParser, to_append: &str) {
        let to_append = to_append.to_string();
        let (toc_str, to_append) = parser.extract_toc(to_append);
        self.set_toc(&toc_str);

        self.doc.push_str(to_append.as_str());
        self.doc.push('\n');
    }

    fn set_toc(&mut self, toc_str: &str) {
        if toc_str.is_empty() {
            return;
        }

        if self.toc.is_some() {
            panic!("Two `docpp:toc` sections for {}", self.full_name());
        }
        let ts = TokenStream::from_str(toc_str).unwrap();
        let document = syn::parse2::<TocDocument>(ts).unwrap();
        self.toc = Some(document);
    }

    pub fn add_submodule(&mut self, sub: Rc<RefCell<DocNodeCommon>>) {
        match self.node {
            DocNode::Module(ref mut m) => m.add_submodule(sub),
            _ => panic!("Cannot add submodule to {}", self.node_type()),
        }
    }

    pub fn add_struct(&mut self, s: Rc<RefCell<DocNodeCommon>>) {
        match self.node {
            DocNode::Module(ref mut m) => m.add_struct(s),
            _ => panic!("Cannot add struct to {}", self.node_type()),
        }
    }

    pub fn add_function(&mut self, f: Rc<RefCell<DocNodeCommon>>) {
        match self.node {
            DocNode::Module(ref mut m) => m.add_function(f),
            _ => panic!("Cannot add function to {}", self.node_type()),
        }
    }

    pub fn add_field(&mut self, f: Rc<RefCell<DocNodeCommon>>) {
        match self.node {
            DocNode::Struct(ref mut s) => s.add_field(f),
            _ => panic!("Cannot add field to {}", self.node_type()),
        }
    }

    /// Dump this document node and all its children
    pub fn dump(&self) {
        let full_name = self.full_name();
        println!("{}: {}", self.node_type(), full_name);
        if let Some(toc) = &self.toc {
            println!("  toc:{{");
            toc.dump(1);
            println!("\n}}");
        }
        println!("  doc:{{\n{}\n}}", self.doc);

        match &self.node {
            DocNode::Module(m) => m.dump(),
            DocNode::Struct(s) => s.dump(),
            _ => {} // Do nothing
        }
    }
}

/// A Rust module with any relevant attributes and related objects.
///
/// This keeps track of the relevant documentation, TOC and objects associated
/// with a Rust module.
pub struct Module {
    functions: HashMap<String, Rc<RefCell<DocNodeCommon>>>,
    sub_modules: HashMap<String, Rc<RefCell<DocNodeCommon>>>,
    structs: HashMap<String, Rc<RefCell<DocNodeCommon>>>,
}

impl Module {
    #[must_use]
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
            sub_modules: HashMap::new(),
            structs: HashMap::new(),
        }
    }

    fn add_function(&mut self, function: Rc<RefCell<DocNodeCommon>>) {
        let name = function.borrow().name.clone();
        self.functions.insert(name, function);
    }

    fn add_struct(&mut self, s: Rc<RefCell<DocNodeCommon>>) {
        let name = s.borrow().name.clone();
        self.structs.insert(name, s);
    }

    fn add_submodule(&mut self, m: Rc<RefCell<DocNodeCommon>>) {
        let name = m.borrow().name.clone();
        self.sub_modules.insert(name, m);
    }

    fn dump(&self) {
        for f in self.functions.values() {
            f.borrow().dump();
        }

        for s in self.structs.values() {
            s.borrow().dump();
        }

        for m in self.sub_modules.values() {
            m.borrow().dump();
        }
    }
}

impl Default for Module {
    fn default() -> Self {
        Self::new()
    }
}

/// A Rust function.
///
/// A function with its associated documentation.
pub struct Function;

impl Function {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for Function {
    fn default() -> Self {
        Self::new()
    }
}

/// A Rust struct.
///
/// A struct with its documentation attribute and fields.
pub struct Struct {
    fields: HashMap<String, Rc<RefCell<DocNodeCommon>>>,
}

impl Struct {
    #[must_use]
    pub fn new() -> Self {
        Self {
            fields: HashMap::new(),
        }
    }

    fn add_field(&mut self, f: Rc<RefCell<DocNodeCommon>>) {
        let name = f.borrow().name.clone();
        self.fields.insert(name, f);
    }

    fn dump(&self) {
        for field in self.fields.values() {
            field.borrow().dump();
        }
    }
}

impl Default for Struct {
    fn default() -> Self {
        Self::new()
    }
}

/// A Rust field within a struct.
pub struct Field;

impl Field {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for Field {
    fn default() -> Self {
        Self::new()
    }
}
