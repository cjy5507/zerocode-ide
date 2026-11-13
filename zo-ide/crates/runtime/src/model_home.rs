//! Whose account a model's requests are sent on.
//!
//! A session's client is built in one order (`zo-ide`'s
//! `build_provider_client`): the model catalog the person edits is asked
//! first, then the providers the person declared themselves, then the name.
//! Whatever asks "is this the session's provider?" — the fast model a probe
//! may ask, the family a helper's model must belong to, the verifier that
//! stays on the session's provider — has to read in that order and give the
//! same answer, down to *which* built-in provider it is. An answer that only
//! tells "a person's own" from "built-in" and then reads the name sends a
//! session the catalog put on Google to `OpenAI`'s fast model.
//!
//! [`ModelHome`] is that one reading. It names no model and builds no client.

use api::ProviderKind;

use crate::model_catalog::ModelCatalog;

/// Where a model's requests go, as the session's client is built for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelHome {
    /// A provider the person declared themselves, by the name they gave it.
    /// Its endpoint speaks the `OpenAI`-compatible wire, and that is all it
    /// shares with `OpenAI`.
    Own(String),
    /// One of the built-in providers.
    BuiltIn(ProviderKind),
    /// Neither the catalog, nor a provider of the person's, nor the name says:
    /// the client is built for whichever login the machine holds. Nothing may
    /// be concluded about such a model's provider from the name.
    Unknown,
}

impl ModelHome {
    /// [`Self::read`] with the catalog as it is on disk now. A catalog file
    /// that cannot be read is no catalog, as it is for the session's client.
    #[must_use]
    pub fn of(model: &str) -> Self {
        Self::read(ModelCatalog::load().ok().as_ref(), model)
    }

    /// The home of `model` under `catalog`, in the order the session's client
    /// is built:
    ///
    /// 1. the catalog — a row of its own or the person's, or a
    ///    `<catalog key>/<id>` selection, which is that key's provider
    ///    whatever the id reads as. It steps aside for a slash id a provider
    ///    of the person's owns ([`ModelCatalog::provider_for_model`]);
    /// 2. a provider the person declared, for the alias-resolved name. A bare
    ///    id the shipped registry names stays first-party even when such a
    ///    provider lists it too ([`api::custom_provider_for_model`]);
    /// 3. the name ([`api::metadata_for_model`]). A `grok…` name is xAI's
    ///    whether or not that adapter is switched on: switched off, no client
    ///    is built for the name at all, and it has no other home.
    ///
    /// Past that the client would be built from the logins and the
    /// environment of the machine ([`api::detect_provider_kind`]): that is
    /// [`Self::Unknown`], and it is not read here.
    #[must_use]
    pub fn read(catalog: Option<&ModelCatalog>, model: &str) -> Self {
        let model = model.trim();
        if let Some(kind) = catalog.and_then(|catalog| catalog.provider_for_model(model)) {
            return Self::BuiltIn(kind);
        }
        let resolved = api::resolve_model_alias(model);
        if let Some(own) = api::custom_provider_for_model(&resolved) {
            return Self::Own(own.config.provider_name.to_string());
        }
        api::metadata_for_model(&resolved)
            .map(|metadata| metadata.provider)
            .or_else(|| api::explicit_non_claude_provider_kind(&resolved))
            .map_or(Self::Unknown, Self::BuiltIn)
    }

    /// The built-in provider this is, when it is one.
    #[must_use]
    pub const fn built_in(&self) -> Option<ProviderKind> {
        match self {
            Self::BuiltIn(kind) => Some(*kind),
            Self::Own(_) | Self::Unknown => None,
        }
    }

    /// Whether a request for a model at `other` is known to go where this
    /// one's goes: the same provider of the person's, or the same built-in
    /// one. Two unknown models are not known to share anything.
    #[must_use]
    pub fn is_where(&self, other: &Self) -> bool {
        !matches!(self, Self::Unknown) && self == other
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::model_catalog::CatalogProvider;

    /// One environment variable as the test found it, put back on drop.
    struct Var(&'static str, Option<OsString>);

    impl Var {
        fn unset(name: &'static str) -> Self {
            let before = std::env::var_os(name);
            std::env::remove_var(name);
            Self(name, before)
        }
    }

    impl Drop for Var {
        fn drop(&mut self) {
            match self.1.take() {
                Some(before) => std::env::set_var(self.0, before),
                None => std::env::remove_var(self.0),
            }
        }
    }

    /// The person's own providers as the test found them, put back on drop.
    struct OwnProviders(Option<OsString>);

    impl OwnProviders {
        /// Declare `providers` (the `ZO_CUSTOM_PROVIDERS` JSON) for this process.
        fn declared(providers: &str) -> Self {
            let before = std::env::var_os(api::CUSTOM_PROVIDERS_ENV);
            std::env::set_var(api::CUSTOM_PROVIDERS_ENV, providers);
            api::refresh_custom_providers_from_env();
            Self(before)
        }
    }

    impl Drop for OwnProviders {
        fn drop(&mut self) {
            match self.0.take() {
                Some(before) => std::env::set_var(api::CUSTOM_PROVIDERS_ENV, before),
                None => std::env::remove_var(api::CUSTOM_PROVIDERS_ENV),
            }
            api::refresh_custom_providers_from_env();
        }
    }

    const OWN: &str = r#"[{"name":"zqown","base_url":"https://own.invalid/v1","models":["gpt-zq-oss","claude-zq-gateway","gemini-zq-relay","zq-own-model","zq-both-list","anthropic/zq-slash-id"],"requires_auth":false}]"#;

    fn own() -> ModelHome {
        ModelHome::Own("zqown".to_string())
    }

    /// A catalog in a fresh home holding rows the person wrote by hand: one
    /// per built-in provider, none spelled as that provider's models are,
    /// and one more that a provider of the person's lists too.
    fn catalog(home: &std::path::Path) -> ModelCatalog {
        let mut catalog = ModelCatalog::load_from_home(home).expect("an empty home loads");
        for (provider, id) in [
            (CatalogProvider::Google, "zq-row-on-google"),
            (CatalogProvider::Anthropic, "zq-row-on-anthropic"),
            (CatalogProvider::Openai, "zq-row-on-openai"),
            (CatalogProvider::Google, "zq-both-list"),
        ] {
            catalog.add(provider, id, id).expect("the row is added");
        }
        ModelCatalog::load_from_home(home).expect("the written catalog loads")
    }

    #[test]
    fn the_catalog_is_asked_first_and_says_which_built_in_provider() {
        let _lock = crate::test_env_lock();
        let _ollama = Var::unset("OLLAMA_BASE_URL");
        let _own = OwnProviders::declared(OWN);
        let home = tempfile::tempdir().expect("a temp home");
        let catalog = catalog(home.path());
        let read = |model: &str| ModelHome::read(Some(&catalog), model);

        // A row the person wrote, under each provider: the name says nothing.
        assert_eq!(read("zq-row-on-google"), ModelHome::BuiltIn(ProviderKind::Google));
        assert_eq!(read("zq-row-on-anthropic"), ModelHome::BuiltIn(ProviderKind::Anthropic));
        assert_eq!(read("zq-row-on-openai"), ModelHome::BuiltIn(ProviderKind::OpenAi));
        // A row a provider of the person's lists too is still the catalog's.
        assert_eq!(read("zq-both-list"), ModelHome::BuiltIn(ProviderKind::Google));
        // `<catalog key>/<id>` is that key's provider, whatever the id reads as.
        assert_eq!(read("openai/gemini-zq-9"), ModelHome::BuiltIn(ProviderKind::OpenAi));
        assert_eq!(read("google/zq-own-model"), ModelHome::BuiltIn(ProviderKind::Google));
        assert_eq!(read("claude/gpt-zq-9"), ModelHome::BuiltIn(ProviderKind::Anthropic));
        // Without the catalog the same rows are nobody's.
        assert_eq!(ModelHome::read(None, "zq-row-on-google"), ModelHome::Unknown);
    }

    #[test]
    fn a_provider_the_person_declared_is_asked_before_the_name() {
        let _lock = crate::test_env_lock();
        let _ollama = Var::unset("OLLAMA_BASE_URL");
        let home = tempfile::tempdir().expect("a temp home");
        let catalog = catalog(home.path());
        let read = |model: &str| ModelHome::read(Some(&catalog), model);

        // Undeclared, the names read as the built-in families they are
        // spelled like.
        {
            let _none = OwnProviders::declared("[]");
            assert_eq!(read("gpt-zq-oss"), ModelHome::BuiltIn(ProviderKind::OpenAi));
            assert_eq!(read("claude-zq-gateway"), ModelHome::BuiltIn(ProviderKind::Anthropic));
            assert_eq!(read("gemini-zq-relay"), ModelHome::BuiltIn(ProviderKind::Google));
            // Whether or not the xAI adapter is switched on in this process.
            assert_eq!(read("grok-zq-9"), ModelHome::BuiltIn(ProviderKind::Xai));
            assert_eq!(read("zq-own-model"), ModelHome::Unknown);
        }

        let _own = OwnProviders::declared(OWN);
        for served in [
            "gpt-zq-oss",
            "claude-zq-gateway",
            "gemini-zq-relay",
            "zq-own-model",
            "zqown/zq-own-model",
            "zqown/gpt-zq-oss",
            // A slash id the provider declares whole is the provider's: the
            // catalog steps aside for it.
            "anthropic/zq-slash-id",
        ] {
            assert_eq!(read(served), own(), "{served}");
        }
    }

    #[test]
    fn a_first_party_id_a_persons_provider_also_lists_stays_first_party() {
        let _lock = crate::test_env_lock();
        let _ollama = Var::unset("OLLAMA_BASE_URL");
        let bare = api::builtin_provider_catalog()
            .iter()
            .find(|entry| entry.provider == ProviderKind::Anthropic)
            .expect("an Anthropic row ships")
            .canonical_model_id;
        let _own = OwnProviders::declared(&format!(
            r#"[{{"name":"zqown","base_url":"https://own.invalid/v1","models":["{bare}"],"requires_auth":false}}]"#
        ));
        let home = tempfile::tempdir().expect("a temp home");
        let catalog = ModelCatalog::load_from_home(home.path()).expect("an empty home loads");

        for catalog in [Some(&catalog), None] {
            assert_eq!(
                ModelHome::read(catalog, bare),
                ModelHome::BuiltIn(ProviderKind::Anthropic),
                "the bare id, with the catalog {}",
                if catalog.is_some() { "read" } else { "unread" }
            );
            // `<provider>/<id>` is how the person opts into their own.
            assert_eq!(ModelHome::read(catalog, &format!("zqown/{bare}")), own());
        }
    }

    #[test]
    fn a_name_nothing_knows_is_unknown_whatever_the_machine_is_logged_into() {
        let _lock = crate::test_env_lock();
        let _ollama = Var::unset("OLLAMA_BASE_URL");
        let _own = OwnProviders::declared("[]");
        let home = tempfile::tempdir().expect("a temp home");
        let catalog = catalog(home.path());

        for unknown in ["zq-model-nobody-lists", "zqrelay/some-model", ""] {
            assert_eq!(ModelHome::read(Some(&catalog), unknown), ModelHome::Unknown, "{unknown:?}");
        }
        // A word before a slash that is neither a catalog key nor a provider
        // of the person's names nobody: the id after it is read by its name,
        // and that is the provider the client is built for.
        assert_eq!(
            ModelHome::read(Some(&catalog), "zqrelay/gpt-zq-9"),
            ModelHome::BuiltIn(ProviderKind::OpenAi)
        );
    }

    #[test]
    fn two_models_share_a_home_only_where_both_are_known() {
        let google = ModelHome::BuiltIn(ProviderKind::Google);
        assert!(google.is_where(&ModelHome::BuiltIn(ProviderKind::Google)));
        assert!(!google.is_where(&ModelHome::BuiltIn(ProviderKind::OpenAi)));
        assert!(own().is_where(&own()));
        assert!(!own().is_where(&ModelHome::Own("zqother".to_string())));
        // A person's provider speaks the wire OpenAI speaks and is not OpenAI.
        assert!(!own().is_where(&ModelHome::BuiltIn(ProviderKind::OpenAi)));
        assert!(!ModelHome::Unknown.is_where(&ModelHome::Unknown));
        assert_eq!(google.built_in(), Some(ProviderKind::Google));
        assert_eq!(own().built_in(), None);
        assert_eq!(ModelHome::Unknown.built_in(), None);
    }
}
