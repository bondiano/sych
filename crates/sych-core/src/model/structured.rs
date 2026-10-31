//! Strict structured output: whole-text JSON, typed validation, one repair.

use serde::de::DeserializeOwned;

use super::{Message, Speaker};

/// Parses the complete reply as `T`. Only a single code fence wrapping the whole reply is
/// accepted around the JSON; JSON is never fished out of prose.
pub fn parse_output<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .unwrap_or(trimmed);
    let mut de = serde_json::Deserializer::from_str(body);
    let value = serde_path_to_error::deserialize(&mut de).map_err(|e| {
        let path = e.path().to_string();
        format!("at `{path}`: {}", e.into_inner())
    })?;
    de.end().map_err(|e| format!("trailing content: {e}"))?;
    Ok(value)
}

/// The repair turn: the original reply and its error, under the same contract and scope.
pub fn repair_messages(messages: &[Message], reply: &str, error: &str) -> Vec<Message> {
    let mut out = messages.to_vec();
    out.push(Message {
        speaker: Speaker::Assistant,
        content: reply.to_owned(),
    });
    out.push(Message {
        speaker: Speaker::User,
        content: format!(
            "Your reply does not match the output contract: {error}\n\
             Return only the corrected JSON document for the same task. Do not add claims \
             or references that were not in your reply or in the data."
        ),
    });
    out
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Out {
        claim: String,
    }

    #[test]
    fn accepts_whole_json_and_a_wrapping_fence() {
        let want = Out { claim: "x".into() };
        assert_eq!(parse_output::<Out>(r#" {"claim":"x"} "#), Ok(want));
        let fenced = "```json\n{\"claim\":\"x\"}\n```";
        assert!(parse_output::<Out>(fenced).is_ok());
    }

    #[test]
    fn rejects_prose_unknown_fields_and_trailing_text() {
        assert!(parse_output::<Out>(r#"Here: {"claim":"x"}"#).is_err());
        assert!(parse_output::<Out>(r#"{"claim":"x"} done"#).is_err());
        let err = parse_output::<Out>(r#"{"claim":"x","extra":1}"#).expect_err("unknown field");
        assert!(err.contains("extra"), "{err}");
    }
}
