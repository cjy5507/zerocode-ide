//! This contract covers the price and usage paths. Model selection and quota
//! policies have their own source contracts.

use std::{fs, path::Path};

use proc_macro2::{TokenStream, TokenTree};
use syn::{Attribute, ImplItem, Item, LitStr, visit::Visit};

const RUST_SOURCES: &[&str] = &[
    "crates/model-prices/src/lib.rs",
    "crates/zerocode-core/src/usage_stats.rs",
    "crates/zerocode-core/src/usage_stats_codex.rs",
    "crates/zerocode-core/src/usage_ledger.rs",
    "crates/zerocode-core/src/usage_stats_opencode.rs",
    "crates/zerocode-shell/src/claude_tokens.rs",
    "crates/zerocode-shell/src/codex_tokens.rs",
    "zo-ide/crates/core-types/src/usage/mod.rs",
    "zo-ide/crates/core-types/src/usage/pricing.rs",
    "zo-ide/crates/core-types/src/usage/dashboard.rs",
];

fn test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Path>()
                .is_ok_and(|path| path.is_ident("test"))
    })
}

#[derive(Default)]
struct ModelLiterals {
    found: Vec<String>,
}

impl ModelLiterals {
    fn tokens(&mut self, tokens: TokenStream) {
        for token in tokens {
            match token {
                TokenTree::Group(group) => self.tokens(group.stream()),
                TokenTree::Literal(literal) => {
                    if let Ok(value) = syn::parse_str::<LitStr>(&literal.to_string()) {
                        self.visit_lit_str(&value);
                    }
                }
                _ => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for ModelLiterals {
    fn visit_item(&mut self, node: &'ast Item) {
        let attrs = match node {
            Item::Const(item) => &item.attrs,
            Item::Fn(item) => &item.attrs,
            Item::Impl(item) => &item.attrs,
            Item::Macro(item) => &item.attrs,
            Item::Mod(item) => &item.attrs,
            Item::Static(item) => &item.attrs,
            _ => return syn::visit::visit_item(self, node),
        };
        if !test_only(attrs) {
            syn::visit::visit_item(self, node);
        }
    }

    fn visit_impl_item(&mut self, node: &'ast ImplItem) {
        let attrs = match node {
            ImplItem::Fn(item) => &item.attrs,
            ImplItem::Const(item) => &item.attrs,
            _ => return syn::visit::visit_impl_item(self, node),
        };
        if !test_only(attrs) {
            syn::visit::visit_impl_item(self, node);
        }
    }

    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        if !attr.path().is_ident("doc") {
            syn::visit::visit_attribute(self, attr);
        }
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.tokens(node.tokens.clone());
    }

    fn visit_lit_str(&mut self, node: &'ast LitStr) {
        let value = node.value();
        if value
            .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '.')))
            .any(names_version)
        {
            self.found.push(value);
        }
    }
}

fn names_version(word: &str) -> bool {
    let word = word.to_ascii_lowercase();
    let lineups = [
        "claude-",
        "gpt-",
        "gemini-",
        "grok-",
        "deepseek-",
        "opus-",
        "sonnet-",
        "haiku-",
        "fable-",
        "mythos-",
    ];
    lineups.iter().any(|prefix| {
        word.strip_prefix(prefix)
            .is_some_and(|suffix| suffix.as_bytes().iter().any(u8::is_ascii_digit))
    }) || matches!(
        word.as_str(),
        "deepseek-chat" | "deepseek-reasoner" | "o1" | "o3" | "o4"
    )
}

#[test]
fn shipped_pricing_paths_contain_no_model_names() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = Vec::new();
    for path in RUST_SOURCES {
        let code = fs::read_to_string(root.join(path)).unwrap();
        let parsed = syn::parse_file(&code).unwrap();
        let mut literals = ModelLiterals::default();
        literals.visit_file(&parsed);
        found.extend(
            literals
                .found
                .into_iter()
                .map(|literal| format!("{path}: {literal}")),
        );
    }
    // These files also contain account-limit policies, tracked separately.
    for (path, function) in [
        ("ui/shell-status.js", "formatUsageCost"),
        ("ui/shell-doc.js", "tokenUsageCost"),
    ] {
        let source = fs::read_to_string(root.join(path)).unwrap();
        let (body, _) = source
            .split_once(&format!("function {function}("))
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap();
        found.extend(
            body.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '.')))
                .filter(|word| names_version(word))
                .map(|word| format!("{path}: {word}")),
        );
    }
    assert!(
        found.is_empty(),
        "Put the model ID in the catalog:\n{}",
        found.join("\n")
    );
}

#[test]
fn a_test_item_does_not_hide_production_code_below_it() {
    let file = syn::parse_file(
        r#"
        #[cfg(test)]
        fn helper() { let _ = "gpt-5.1"; }
        fn shipped() { println!("claude-3-opus"); }
    "#,
    )
    .unwrap();
    let mut literals = ModelLiterals::default();
    literals.visit_file(&file);
    assert_eq!(literals.found, ["claude-3-opus"]);
}
