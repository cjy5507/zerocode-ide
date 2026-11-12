//! One request to the session provider's fast-tier model, outside any turn.
//!
//! Two features ask a small question of a small model while the session does
//! something else: the fold of an over-cap tool result (`tool_digest`), and
//! the lessons a pass draws from what the person said (`zo auto-dream`). Both
//! dial the model the way a sub-agent is dialled and hold the request to a
//! wall clock, and neither may be load-bearing: every failure here is a
//! `String` the caller turns into "do without".

use std::time::Duration;

/// The fast-tier model of the provider a session runs on.
pub struct FastModel {
    model: String,
    client: api::ProviderClient,
}

impl FastModel {
    /// The fast model for a session whose own model is `parent_model`.
    ///
    /// # Errors
    /// When no client can be built for it: no credential, an unknown model.
    pub fn for_parent(parent_model: Option<&str>) -> Result<Self, String> {
        let model = Self::named_for(parent_model);
        let client = crate::misc_tools::build_provider_client_for_agent(&model)?;
        Ok(Self { model, client })
    }

    /// The fast model [`Self::for_parent`] would dial for `parent_model`, by
    /// name alone: no client is built and no credential is read. The same
    /// provider's, always — a provider with no fast tier answers on the
    /// parent's own model.
    #[must_use]
    pub fn named_for(parent_model: Option<&str>) -> String {
        // The cheap-model choice is a family alias (`haiku`); the wire wants
        // the release id, the same way the sub-agent client resolves it.
        api::resolve_model_alias(&crate::fanout::decompose_model(parent_model))
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
            model: self.model.clone(),
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
