//! The tools' door to [`runtime::model_home`]: whose account a model's
//! requests are sent on, read in the order the session's client is built.
//!
//! Every judgment here that asks "is this the session's provider?" comes
//! through this door, so that none reads the name on its own. A caller that
//! asks about several models loads the catalog once ([`Homes`]).

use runtime::model_catalog::ModelCatalog;
use runtime::model_home::ModelHome;

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

    /// Where `model`'s requests go.
    pub(crate) fn of(&self, model: &str) -> ModelHome {
        ModelHome::read(self.0.as_ref(), model)
    }

    /// Whether requests for `left` and for `right` are known to go to the
    /// same place: one provider of the person's, or one built-in provider.
    pub(crate) fn same(&self, left: &str, right: &str) -> bool {
        self.of(left).is_where(&self.of(right))
    }
}
