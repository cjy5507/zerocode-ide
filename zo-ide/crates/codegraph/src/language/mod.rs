use std::path::Path;

use tree_sitter::{Language, Node};

mod go;
mod python;
mod rust;
mod typescript;

static BUILT_IN_SPECS: [&dyn LanguageSpec; 4] = [
    &rust::RUST,
    &typescript::TYPESCRIPT,
    &python::PYTHON,
    &go::GO,
];

/// The complete language-specific surface used by the generic extractor.
///
/// Adding a language means implementing this trait in one module and adding
/// that spec to `built_in_specs`; the scanner and extractor contain no
/// language-name or extension branches.
pub trait LanguageSpec: Send + Sync {
    fn name(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
    fn dialect(&self, path: &Path) -> &'static str;
    fn language(&self, path: &Path) -> Language;
    fn definitions_query(&self, path: &Path) -> &'static str;
    fn imports_query(&self, path: &Path) -> &'static str;
    fn references_query(&self, path: &Path) -> &'static str;

    /// Cheap enclosing-scope lookup for a captured definition name.
    fn container_name(&self, _node: Node<'_>, _source: &[u8]) -> Option<String> {
        None
    }

    fn matches_path(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| self.extensions().contains(&extension))
    }
}

pub(crate) fn spec_for_path(path: &Path) -> Option<&'static dyn LanguageSpec> {
    built_in_specs()
        .iter()
        .copied()
        .find(|spec| spec.matches_path(path))
}

pub(crate) const fn built_in_specs() -> &'static [&'static dyn LanguageSpec] {
    &BUILT_IN_SPECS
}

pub(crate) fn node_text(node: Node<'_>, source: &[u8]) -> Option<String> {
    node.utf8_text(source).ok().map(str::to_owned)
}

pub(crate) fn ancestor_name_by_field(
    node: Node<'_>,
    source: &[u8],
    containers: &[(&str, &str)],
) -> Option<String> {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if let Some((_, field)) = containers
            .iter()
            .find(|(kind, _)| *kind == current.kind())
        {
            if let Some(name) = current.child_by_field_name(field) {
                if name.id() != node.id() {
                    return node_text(name, source);
                }
            }
        }
        ancestor = current.parent();
    }
    None
}

pub(crate) fn descendant_text_by_kind(
    node: Node<'_>,
    source: &[u8],
    kind: &str,
) -> Option<String> {
    if node.kind() == kind {
        return node_text(node, source);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(text) = descendant_text_by_kind(child, source, kind) {
            return Some(text);
        }
    }
    None
}
