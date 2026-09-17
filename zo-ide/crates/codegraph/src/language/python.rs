use std::path::Path;

use tree_sitter::{Language, Node};

use super::{ancestor_name_by_field, LanguageSpec};

pub(super) static PYTHON: PythonSpec = PythonSpec;

pub(super) struct PythonSpec;

impl LanguageSpec for PythonSpec {
    fn name(&self) -> &'static str {
        "python"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["py"]
    }

    fn dialect(&self, _path: &Path) -> &'static str {
        "python"
    }

    fn language(&self, _path: &Path) -> Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn definitions_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/python/definitions.scm")
    }

    fn imports_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/python/imports.scm")
    }

    fn references_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/python/references.scm")
    }

    fn container_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        ancestor_name_by_field(node, source, &[("class_definition", "name")])
    }
}
