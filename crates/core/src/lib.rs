//! Core model for Sendito: requests, variables, resolution and scripts.
//!
//! Nothing in this crate touches the network, the terminal or the file system
//! (except reading file contents referenced by bodies at resolution time).

pub mod complete;
pub mod inherit;
pub mod json_schema;
pub mod model;
pub mod resolve;
pub mod script;
pub mod template;
pub mod vars;

pub use complete::FieldNode;
pub use model::*;
pub use vars::{Scope, VarValue};
