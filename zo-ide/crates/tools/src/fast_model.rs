//! One request to the session provider's fast-tier model, outside any turn.
//!
//! The fold of an over-cap tool result (`tool_digest`) asks a small model
//! something while the session does something else. It dials the model the
//! way a sub-agent is dialled and holds the request to a wall clock, and it
//! may not be load-bearing: every failure here is a `String` the caller
//! turns into "do without".
//!
//! It hands over what the session holds — a tool's whole output — so there
//! is one way in, and it names a model only where the provider the session
//! runs on has a fast tier of its own. A session on a person's own endpoint
//! has the model that endpoint's `providers` row names (`fast_model`). One
//! whose row names none, on a local model, or on a name the catalog does not
//! know has none, and is asked nothing: its "fast model" would be a built-in
//! provider's, on that provider's account.
//!
//! Until v1.1.148 a second caller came this way: a pass that asked what the
//! person's words in a session teach (`dreamLessons`). It was taken out.

use std::time::Duration;

/// The fast-tier model of the provider a session runs on.
pub struct FastModel {
    model: String,
    client: api::ProviderClient,
}

impl FastModel {
    /// The fast-tier model of the provider a session on `parent_model` runs
    /// on, by name alone — no client is built and no credential is read — or
    /// `None` where that provider has no fast tier that is its own.
    #[must_use]
    pub fn fast_tier_named_for(parent_model: Option<&str>) -> Option<String> {
        // The cheap-model choice is a family alias (`haiku`); the wire wants
        // the release id, the same way the sub-agent client resolves it.
        crate::fanout::fast_tier_model(parent_model).map(|model| api::resolve_model_alias(&model))
    }

    /// The client for `model`, a name [`Self::fast_tier_named_for`] gave for
    /// a session on `session_model` — on the login that session's own client
    /// is pinned to, where it is pinned to one
    /// (`build_provider_client_for_the_sessions_fast_model`).
    ///
    /// # Errors
    /// When no client can be built for it: no credential, an unknown model.
    pub(crate) fn dialling(model: String, session_model: Option<&str>) -> Result<Self, String> {
        let client = match session_model {
            Some(session) => {
                crate::misc_tools::build_provider_client_for_the_sessions_fast_model(&model, session)?
            }
            None => crate::misc_tools::build_provider_client_for_agent(&model)?,
        };
        Ok(Self { model, client })
    }

    /// [`Self::fast_tier_named_for`] for a session on `parent_model` whose
    /// home the caller holds: the context a tool runs in knows where the
    /// client it runs on was built to send.
    pub(crate) fn fast_tier_named_where(
        parent_model: &str,
        home: &crate::model_home::ModelHome,
    ) -> Option<String> {
        crate::fanout::fast_tier_model_where(parent_model, home)
            .map(|model| api::resolve_model_alias(&model))
    }

    /// The model a reply is attributed to.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The text of the reply to `system` + `user`, bounded by `max_tokens` and
    /// by `within` on the wall clock.
    ///
    /// # Errors
    /// When the request fails, or outlasts `within`.
    pub fn reply(
        &self,
        system: &[String],
        user: &str,
        max_tokens: u32,
        within: Duration,
    ) -> Result<String, String> {
        let request = api::MessageRequest {
            // The id the endpoint knows the model by: a provider of the
            // person's is named `<provider>/<id>` here, and is asked for `<id>`.
            model: api::wire_model_id(&self.model),
            max_tokens: max_tokens.min(api::max_tokens_for_model(&self.model)),
            messages: vec![api::InputMessage::user_text(user)],
            // The identity-first split the sub-agent path uses: the Claude Max
            // OAuth route rejects any other first system block.
            system: Some(runtime::split_system_with_identity(&system.join("\n\n"))),
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        };
        let response = api::sync_bridge::run_blocking(async {
            tokio::time::timeout(within, self.client.send_message(&request)).await
        })
        .map_err(|_| format!("timed out after {}s", within.as_secs()))?
        .map_err(|error| error.to_string())?;
        Ok(response
            .content
            .into_iter()
            .filter_map(|block| match block {
                api::OutputContentBlock::Text { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}
