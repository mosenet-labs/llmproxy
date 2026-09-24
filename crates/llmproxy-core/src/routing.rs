use serde_json::Value;

use crate::protocol::Protocol;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Route {
    Proxy(Protocol),
    Auto,
    MethodNotAllowed,
    NotFound,
}

pub fn match_route(method: &str, path: &str) -> Route {
    let route = match path {
        "/v1/chat/completions" => Route::Proxy(Protocol::OpenAiChat),
        "/v1/responses" => Route::Proxy(Protocol::OpenAiResponses),
        "/v1/messages" => Route::Proxy(Protocol::AnthropicMessages),
        "/v1/auto" => Route::Auto,
        _ => return Route::NotFound,
    };
    if method == "POST" {
        route
    } else {
        Route::MethodNotAllowed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoError {
    InvalidBody,
    Ambiguous,
}

/// Classifies native request JSON without changing its content.
/// Anthropic requires its protocol header because `messages` is shared with Chat.
pub fn classify_auto(anthropic_version: bool, body: &Value) -> Result<Protocol, AutoError> {
    let object = body.as_object().ok_or(AutoError::InvalidBody)?;
    let has_input = object.contains_key("input");
    let has_messages = object.contains_key("messages");
    if has_input == has_messages {
        return Err(AutoError::Ambiguous);
    }
    if anthropic_version {
        if has_messages {
            Ok(Protocol::AnthropicMessages)
        } else {
            Err(AutoError::Ambiguous)
        }
    } else if has_input {
        Ok(Protocol::OpenAiResponses)
    } else {
        Ok(Protocol::OpenAiChat)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn matches_only_known_post_routes() {
        assert_eq!(
            match_route("POST", "/v1/chat/completions"),
            Route::Proxy(Protocol::OpenAiChat)
        );
        assert_eq!(match_route("POST", "/v1/auto"), Route::Auto);
        assert_eq!(match_route("GET", "/v1/messages"), Route::MethodNotAllowed);
        assert_eq!(match_route("POST", "/v1/unknown"), Route::NotFound);
    }

    #[test]
    fn auto_classification_rejects_ambiguous_requests() {
        assert_eq!(
            classify_auto(false, &json!({"input": "hello"})),
            Ok(Protocol::OpenAiResponses)
        );
        assert_eq!(
            classify_auto(false, &json!({"messages": []})),
            Ok(Protocol::OpenAiChat)
        );
        assert_eq!(
            classify_auto(true, &json!({"messages": []})),
            Ok(Protocol::AnthropicMessages)
        );
        assert_eq!(
            classify_auto(false, &json!({"input": [], "messages": []})),
            Err(AutoError::Ambiguous)
        );
    }
}
