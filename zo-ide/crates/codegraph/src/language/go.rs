use std::path::Path;

use tree_sitter::{Language, Node};

use super::{descendant_text_by_kind, LanguageSpec};

pub(super) static GO: GoSpec = GoSpec;

pub(super) struct GoSpec;

impl LanguageSpec for GoSpec {
    fn name(&self) -> &'static str {
        "go"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn dialect(&self, _path: &Path) -> &'static str {
        "go"
    }

    fn language(&self, _path: &Path) -> Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn definitions_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/go/definitions.scm")
    }

    fn imports_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/go/imports.scm")
    }

    fn references_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/go/references.scm")
    }

    fn container_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        let mut ancestor = node.parent();
        while let Some(current) = ancestor {
            if current.kind() == "method_declaration" {
                return current
                    .child_by_field_name("receiver")
                    .and_then(|receiver| descendant_text_by_kind(receiver, source, "type_identifier"));
            }
            ancestor = current.parent();
        }
        None
    }
}
