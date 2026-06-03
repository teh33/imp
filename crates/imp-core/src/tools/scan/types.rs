//! Schema types for scan results — types, functions, fields extracted from source.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Internal, // pub(crate), package-private, etc.
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Struct,
    Class,
    Interface,
    Enum,
    Trait,
    TypeAlias,
    Union,
    Protocol,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub type_name: String,
    pub optional: bool,
}

#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub name: String,
    pub source: String, // "file:line"
    pub kind: TypeKind,
    pub fields: Vec<Field>,
    pub variants: Vec<String>,
    pub methods: Vec<String>,
    pub visibility: Visibility,
    pub implements: Vec<String>,
}

impl Default for TypeInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            source: String::new(),
            kind: TypeKind::Struct,
            fields: Vec::new(),
            variants: Vec::new(),
            methods: Vec::new(),
            visibility: Visibility::Public,
            implements: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub name: String,
    pub source: String,
    pub signature: String,
    pub visibility: Visibility,
    pub is_async: bool,
    pub is_test: bool,
}

impl Default for FunctionInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            source: String::new(),
            signature: String::new(),
            visibility: Visibility::Public,
            is_async: false,
            is_test: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Contains,
    Implements,
    Imports,
    Calls,
    Tests,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeInfo {
    pub from: String,
    pub to: String,
    pub source: String,
    pub kind: EdgeKind,
    pub label: Option<String>,
}

/// Combined extraction result from one or more files.
#[derive(Debug, Default, Clone)]
pub struct ScanResult {
    pub types: BTreeMap<String, TypeInfo>,
    pub functions: BTreeMap<String, FunctionInfo>,
    pub edges: Vec<EdgeInfo>,
}

impl ScanResult {
    pub fn merge(&mut self, other: ScanResult) {
        self.types.extend(other.types);
        self.functions.extend(other.functions);
        self.edges.extend(other.edges);
    }
}
