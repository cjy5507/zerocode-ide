use std::path::Path;

use tree_sitter::{Language, Node};

use super::{ancestor_name_by_field, LanguageSpec};

pub(super) static TYPESCRIPT: TypeScriptSpec = TypeScriptSpec;

const COMMON_DEFINITIONS_QUERY: &str =
    include_str!("queries/typescript/definitions_common.scm");
const TYPESCRIPT_DEFINITIONS_QUERY: &str = concat!(
    include_str!("queries/typescript/definitions_common.scm"),
    include_str!("queries/typescript/definitions_ts.scm")
);
const COMMON_REFERENCES_QUERY: &str = include_str!("queries/typescript/references_common.scm");
const TYPESCRIPT_REFERENCES_QUERY: &str = concat!(
    include_str!("queries/typescript/references_common.scm"),
    include_str!("queries/typescript/references_ts.scm")
);

pub(super) struct TypeScriptSpec;

impl TypeScriptSpec {
    fn extension(path: &Path) -> &str {
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
    }

    fn is_javascript(path: &Path) -> bool {
        matches!(Self::extension(path), "js" | "jsx" | "mjs" | "cjs")
    }
}

impl LanguageSpec for TypeScriptSpec {
    fn name(&self) -> &'static str {
        "typescript-javascript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "tsx", "js", "jsx", "mjs", "cjs"]
    }

    fn dialect(&self, path: &Path) -> &'static str {
        match Self::extension(path) {
            "tsx" => "tsx",
            "js" | "jsx" | "mjs" | "cjs" => "javascript",
            _ => "typescript",
        }
    }

    fn language(&self, path: &Path) -> Language {
        match Self::extension(path) {
            "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
            "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::LANGUAGE.into(),
            _ => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }

    fn definitions_query(&self, path: &Path) -> &'static str {
        if Self::is_javascript(path) {
            COMMON_DEFINITIONS_QUERY
        } else {
            TYPESCRIPT_DEFINITIONS_QUERY
        }
    }

    fn imports_query(&self, _path: &Path) -> &'static str {
        include_str!("queries/typescript/imports.scm")
    }

    fn references_query(&self, path: &Path) -> &'static str {
        if Self::is_javascript(path) {
            COMMON_REFERENCES_QUERY
        } else {
            TYPESCRIPT_REFERENCES_QUERY
        }
    }

    fn container_name(&self, node: Node<'_>, source: &[u8]) -> Option<String> {
        ancestor_name_by_field(
            node,
            source,
            &[
                ("class_declaration", "name"),
                ("abstract_class_declaration", "name"),
                ("class", "name"),
                ("interface_declaration", "name"),
                ("internal_module", "name"),
            ],
        )
    }
}
