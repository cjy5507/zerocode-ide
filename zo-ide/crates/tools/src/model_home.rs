//! The tools' door to [`runtime::model_home`]: whose account a model's
//! requests are sent on, read in the order the session's client is built.
//!
//! Every judgment here that asks "is this the session's provider?" comes
//! through this door, so that none reads the name on its own. A caller that
//! asks about several models loads the catalog once ([`Homes`]).
//!
//! Two questions come through it, and they are read differently. Where a
//! model the session *runs on* sends is what its client was built by
//! ([`of_the_session`]): the session's own record first, with no file read,
//! so that an edit of the catalog while the session runs moves no judgment
//! off the client. Where a model the session holds no client for *would*
//! send — a candidate for a helper, a verifier — is what the catalog says now
//! ([`Homes::of`]), which is also what its client is then built by.

use runtime::model_catalog::ModelCatalog;

pub(crate) use runtime::model_home::ModelHome;

/// Where the requests of a session running on `model` go, and where those of
/// a helper on `model` would. What the session's client for `model` was
/// built by, when this process's session holds one
/// ([`runtime::model_home::held`] — a home of `Unknown` is such a record, and
/// stands); the catalog as it is now for a model it holds none for. A
/// helper's client is built by the same two-step
/// (`build_provider_client_for_agent`), so a judgment about a helper's model
/// that asks here reads what that client will be built by.
pub(crate) fn of_the_session(model: &str) -> ModelHome {
    runtime::model_home::held(model).map_or_else(|| Homes::load().of(model), |held| held.home)
}

/// One reading of the model catalog the person edits, for as many models as
/// the caller asks about. The file is read when this is made and not again.
pub(crate) struct Homes(Option<ModelCatalog>);

impl Homes {
    /// The catalog as it is on disk now. One that cannot be read is no
    /// catalog, as it is for the session's client.
    pub(crate) fn load() -> Self {
        // A test that never took the crate's environment lock still reads an
        // empty config home of the test process's own, not the developer's.
        #[cfg(test)]
        crate::tests::env_ready();
        Self(ModelCatalog::load().ok())
    }

    /// The catalog that was read, for what else a caller asks of it about
    /// the same model (the login a row is pinned to).
    pub(crate) const fn catalog(&self) -> Option<&ModelCatalog> {
        self.0.as_ref()
    }

    /// Where `model`'s requests would go, as the catalog reads now: the home
    /// of a model no client is built for yet.
    pub(crate) fn of(&self, model: &str) -> ModelHome {
        ModelHome::read(self.0.as_ref(), model)
    }
}
