use serde_json::{Value, json};

use crate::{MAX_RESPONSE_BYTES, Protocol, Provider, ProviderKind, Refusal, valid_model};

pub struct Prepared {
    pub body: Vec<u8>,
    questions: Option<Value>,
}

impl Provider {
    pub fn prepare(&self, body: Vec<u8>) -> Result<Prepared, Refusal> {
        if self.kind == ProviderKind::Jev {
            return Ok(Prepared {
                body,
                questions: None,
            });
        }
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(Refusal::TooLarge);
        }
        let mut request: Value = serde_json::from_slice(&body).map_err(|_| Refusal::Request)?;
        if request.get("model").and_then(Value::as_str) != Some(self.identity().as_str()) {
            return Err(Refusal::SettingsChanged);
        }
        validate_request(&request)?;
        request["model"] = json!(self.model);
        let body = if self.protocol == Protocol::OpenaiJson {
            self.clone().endpoint_validated()?;
            crate::openai::prepare(&request)?
        } else {
            serde_json::to_vec(&request).map_err(|_| Refusal::Request)?
        };
        let questions = request.get_mut("questions").map(Value::take);
        Ok(Prepared { body, questions })
    }

    pub fn decode(&self, prepared: &Prepared, body: Vec<u8>) -> Result<String, Refusal> {
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(Refusal::TooLarge);
        }
        let Some(questions) = &prepared.questions else {
            return String::from_utf8(body).map_err(|_| Refusal::Response);
        };
        let mut response: Value = if self.protocol == Protocol::OpenaiJson {
            crate::openai::decode(questions, &body)?
        } else {
            serde_json::from_slice(&body).map_err(|_| Refusal::Response)?
        };
        validate_answers(questions, &response)?;
        let model = response
            .get("model")
            .and_then(Value::as_str)
            .ok_or(Refusal::Response)?;
        if !valid_model(model) {
            return Err(Refusal::Response);
        }
        let model = format!("{}:{model}", self.identity());
        response["model"] = json!(model);
        response["judgmentProvider"] = json!({
            "kind":self.kind,
            "protocol":self.protocol,
            "model":self.model,
            "confidenceSource":if self.protocol == Protocol::OpenaiJson { "model_self_reported" } else { "provider_reported" },
        });
        if self.protocol == Protocol::OpenaiJson {
            response["judgmentProvider"]["verification"] = json!("user_selected_unverified");
        }
        serde_json::to_string(&response).map_err(|_| Refusal::Response)
    }

    pub fn prepare_served(&self, body: Vec<u8>) -> Result<Prepared, Refusal> {
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(Refusal::TooLarge);
        }
        if self.kind != ProviderKind::Local {
            return Err(Refusal::Settings);
        }
        if self.protocol == Protocol::OpenaiJson {
            return crate::openai::prepare_served(self, &body);
        }
        let mut request: Value = serde_json::from_slice(&body).map_err(|_| Refusal::Request)?;
        if request["model"].as_str() != Some(&self.model) {
            return Err(Refusal::SettingsChanged);
        }
        request["model"] = json!(self.identity());
        self.prepare(serde_json::to_vec(&request).map_err(|_| Refusal::Request)?)
    }
}

fn labels(question: &Value) -> Result<Vec<String>, Refusal> {
    match question.get("type").and_then(Value::as_str) {
        Some("choice") => {
            let criteria = question
                .get("criteria")
                .and_then(Value::as_object)
                .ok_or(Refusal::Request)?;
            if !(2..=255).contains(&criteria.len()) {
                return Err(Refusal::Request);
            }
            Ok(criteria.keys().cloned().collect())
        }
        Some("score") => {
            let criteria = question
                .get("criteria")
                .and_then(Value::as_array)
                .ok_or(Refusal::Request)?;
            if !(2..=10).contains(&criteria.len()) {
                return Err(Refusal::Request);
            }
            Ok((0..criteria.len()).map(|index| index.to_string()).collect())
        }
        _ => Err(Refusal::Request),
    }
}

fn validate_request(request: &Value) -> Result<(), Refusal> {
    if request.get("state").is_none_or(Value::is_null) {
        return Err(Refusal::Request);
    }
    let questions = request
        .get("questions")
        .and_then(Value::as_object)
        .ok_or(Refusal::Request)?;
    if questions.is_empty() || questions.len() > 256 {
        return Err(Refusal::Request);
    }
    for question in questions.values() {
        if question.get("instructions").is_none_or(Value::is_null) {
            return Err(Refusal::Request);
        }
        match question.get("type").and_then(Value::as_str) {
            Some("choice" | "score") => {
                labels(question)?;
            }
            Some("noul") => {}
            _ => return Err(Refusal::Request),
        }
    }
    Ok(())
}

fn probability(value: Option<&Value>) -> Result<f64, Refusal> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && (0.0..=1.0).contains(number))
        .ok_or(Refusal::Response)
}

fn validate_answers(questions: &Value, response: &Value) -> Result<(), Refusal> {
    let usage = response
        .get("usage")
        .and_then(Value::as_object)
        .ok_or(Refusal::Response)?;
    for field in ["input_tokens", "output_tokens"] {
        usage
            .get(field)
            .and_then(Value::as_u64)
            .ok_or(Refusal::Response)?;
    }
    if usage
        .get("truncated")
        .is_some_and(|flag| !flag.is_boolean())
        || usage
            .get("deadline_budget_exceeded")
            .is_some_and(|flag| !flag.is_boolean())
        || usage
            .get("state_tokens_dropped")
            .is_some_and(|count| count.as_u64().is_none())
    {
        return Err(Refusal::Response);
    }
    if usage.get("truncated").and_then(Value::as_bool) == Some(true)
        || usage
            .get("state_tokens_dropped")
            .and_then(Value::as_u64)
            .is_some_and(|count| count > 0)
    {
        return Err(Refusal::Truncated);
    }
    if let Some(truncated) = usage.get("truncated_questions") {
        let truncated = truncated.as_array().ok_or(Refusal::Response)?;
        if !truncated.is_empty() {
            return Err(Refusal::Truncated);
        }
    }
    if usage
        .get("options")
        .and_then(Value::as_object)
        .is_some_and(|options| {
            options
                .values()
                .any(|details| details.get("total") != details.get("distinct"))
        })
    {
        return Err(Refusal::Truncated);
    }
    if usage
        .get("deadline_budget_exceeded")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Err(Refusal::DeadlineBudget);
    }
    let questions = questions.as_object().ok_or(Refusal::Request)?;
    let answers = response
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(Refusal::Response)?;
    if answers.len() != questions.len() {
        return Err(Refusal::Response);
    }
    for (name, question) in questions {
        let answer = answers.get(name).ok_or(Refusal::Response)?;
        if answer.get("type") != question.get("type") {
            return Err(Refusal::Response);
        }
        if question.get("type").and_then(Value::as_str) == Some("noul") {
            probability(answer.get("noul"))?;
            continue;
        }
        probability(answer.get("confidence"))?;
        let options = labels(question)?;
        let probabilities = answer
            .get("probabilities")
            .and_then(Value::as_object)
            .ok_or(Refusal::Response)?;
        if probabilities.len() != options.len() {
            return Err(Refusal::Response);
        }
        let mut total = 0.0;
        let mut greatest = 0.0_f64;
        let mut expected = 0.0;
        for (index, label) in options.iter().enumerate() {
            let chance = probability(probabilities.get(label))?;
            total += chance;
            greatest = greatest.max(chance);
            expected += chance * index as f64;
        }
        let tolerance = options.len() as f64 * 0.000_051 + 0.000_001;
        if (total - 1.0).abs() > tolerance {
            return Err(Refusal::Response);
        }
        if question.get("type").and_then(Value::as_str) == Some("choice") {
            let chosen = answer
                .get("choice")
                .and_then(Value::as_str)
                .ok_or(Refusal::Response)?;
            let chance = probability(probabilities.get(chosen))?;
            if greatest - chance > 0.000_101 {
                return Err(Refusal::Response);
            }
        } else {
            let score = answer
                .get("score")
                .and_then(Value::as_f64)
                .ok_or(Refusal::Response)?;
            if !score.is_finite() || (score - expected).abs() > tolerance * options.len() as f64 {
                return Err(Refusal::Response);
            }
            let legend = answer
                .get("legend")
                .and_then(Value::as_object)
                .ok_or(Refusal::Response)?;
            if legend.len() != options.len()
                || options
                    .iter()
                    .any(|label| legend.get(label).and_then(Value::as_str).is_none())
            {
                return Err(Refusal::Response);
            }
        }
    }
    Ok(())
}
