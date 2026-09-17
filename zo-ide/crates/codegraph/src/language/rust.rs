use std::path::Path;

use tree_sitter::{Language, Node};

use super::{ancestor_name_by_field, LanguageSpec};

pub(super) static RUST: RustSpec = RustSpec;

pub(super) struct RustSpec;

impl LanguageSpec for RustSpec {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn dialect(&self, _path: &Path) -> &'static str {
        "rust"
    }

    fn language(&self, _path: &Path) -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn definitions_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/rust/definitions.scm")
    }

    fn imports_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/rust/imports.scm")
    }

    fn references_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/rust/references.scm")
    }

    fn container_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        ancestor_name_by_field(
            node,
            source,
            &[("impl_item", "type"), ("trait_item", "name"), ("mod_item", "name")],
        )
    }
}
