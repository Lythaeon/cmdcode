use serde::{Deserialize, Serialize};
use std::fmt;

/// Newtype for model identifiers — never bare strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(String);

impl ModelId {
    /// Create a new `ModelId` from any string-like value.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Return the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Strip `command-code/` prefix if present.
    pub fn strip_prefix(&self) -> Self {
        if let Some(rest) = self.0.strip_prefix("command-code/") {
            Self(rest.to_string())
        } else {
            self.clone()
        }
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ModelId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Reasoning effort level — exhaustive enum, no string matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
    /// Extra-high reasoning effort.
    Xhigh,
    /// Maximum reasoning effort.
    Max,
}

impl Effort {
    /// Parse a string into an `Effort`, returning `None` if unrecognized.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    /// Return the string representation of this effort level.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parse `model_id:effort` syntax.
pub fn parse_model_and_effort(raw: &str) -> (ModelId, Option<Effort>) {
    let stripped = raw.strip_prefix("command-code/").unwrap_or(raw);
    if let Some((model, effort_str)) = stripped.rsplit_once(':') {
        if let Some(effort) = Effort::from_str_opt(effort_str) {
            return (ModelId::new(model), Some(effort));
        }
    }
    (ModelId::new(stripped), None)
}

/// Session identifier newtype.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    /// Generate a new random session ID (UUID v4).
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Return the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Request identifier newtype.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    /// Generate a new random request ID (UUID v4, hex-encoded).
    pub fn generate() -> Self {
        let id = uuid::Uuid::new_v4();
        Self(format!("{:x}", id.as_u128()))
    }

    /// Return the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Provider identifier newtype.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderId(String);

impl ProviderId {
    /// Create a new `ProviderId` from any string-like value.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Return the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ProviderId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Context window size in tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContextWindow(u64);

impl ContextWindow {
    /// Create a new `ContextWindow` with the given token count.
    pub fn new(tokens: u64) -> Self {
        Self(tokens)
    }

    /// Return the token count.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for ContextWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 >= 1_000_000 {
            write!(f, "{}M", self.0 / 1_000_000)
        } else if self.0 >= 1_000 {
            write!(f, "{}K", self.0 / 1_000)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_model_id_new() {
        let id = ModelId::new("test-model");
        assert_eq!(id.as_str(), "test-model");
    }

    #[test]
    fn test_model_id_display() {
        let id = ModelId::new("gpt-4");
        assert_eq!(format!("{id}"), "gpt-4");
    }

    #[test]
    fn test_model_id_as_ref() {
        let id = ModelId::new("claude-3");
        let s: &str = id.as_ref();
        assert_eq!(s, "claude-3");
    }

    #[test]
    fn test_model_id_strip_prefix_with_prefix() {
        let id = ModelId::new("command-code/gpt-4");
        let stripped = id.strip_prefix();
        assert_eq!(stripped.as_str(), "gpt-4");
    }

    #[test]
    fn test_model_id_strip_prefix_without_prefix() {
        let id = ModelId::new("gpt-4");
        let stripped = id.strip_prefix();
        assert_eq!(stripped.as_str(), "gpt-4");
    }

    #[test]
    fn test_effort_from_str_opt() {
        assert_eq!(Effort::from_str_opt("low"), Some(Effort::Low));
        assert_eq!(Effort::from_str_opt("medium"), Some(Effort::Medium));
        assert_eq!(Effort::from_str_opt("high"), Some(Effort::High));
        assert_eq!(Effort::from_str_opt("xhigh"), Some(Effort::Xhigh));
        assert_eq!(Effort::from_str_opt("max"), Some(Effort::Max));
        assert_eq!(Effort::from_str_opt("invalid"), None);
        assert_eq!(Effort::from_str_opt(""), None);
        assert_eq!(Effort::from_str_opt("LOW"), Some(Effort::Low));
        assert_eq!(Effort::from_str_opt("High"), Some(Effort::High));
    }

    #[test]
    fn test_effort_as_str() {
        assert_eq!(Effort::Low.as_str(), "low");
        assert_eq!(Effort::Medium.as_str(), "medium");
        assert_eq!(Effort::High.as_str(), "high");
        assert_eq!(Effort::Xhigh.as_str(), "xhigh");
        assert_eq!(Effort::Max.as_str(), "max");
    }

    #[test]
    fn test_effort_display() {
        assert_eq!(format!("{}", Effort::Low), "low");
        assert_eq!(format!("{}", Effort::High), "high");
    }

    #[test]
    fn test_parse_model_and_effort_basic() {
        let (model, effort) = parse_model_and_effort("gpt-4");
        assert_eq!(model.as_str(), "gpt-4");
        assert!(effort.is_none());
    }

    #[test]
    fn test_parse_model_and_effort_with_effort() {
        let (model, effort) = parse_model_and_effort("gpt-4:high");
        assert_eq!(model.as_str(), "gpt-4");
        assert_eq!(effort, Some(Effort::High));
    }

    #[test]
    fn test_parse_model_and_effort_with_prefix() {
        let (model, effort) = parse_model_and_effort("command-code/gpt-4:max");
        assert_eq!(model.as_str(), "gpt-4");
        assert_eq!(effort, Some(Effort::Max));
    }

    #[test]
    fn test_parse_model_and_effort_invalid_effort() {
        let (model, effort) = parse_model_and_effort("gpt-4:invalid");
        assert_eq!(model.as_str(), "gpt-4:invalid");
        assert!(effort.is_none());
    }

    #[test]
    fn test_parse_model_and_effort_empty() {
        let (model, effort) = parse_model_and_effort("");
        assert_eq!(model.as_str(), "");
        assert!(effort.is_none());
    }

    #[test]
    fn test_session_id_generate() {
        let id = SessionId::generate();
        assert!(!id.as_str().is_empty());
        // UUID v4 format: 8-4-4-4-12
        assert_eq!(id.as_str().chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn test_session_id_unique() {
        let id1 = SessionId::generate();
        let id2 = SessionId::generate();
        assert_ne!(id1.as_str(), id2.as_str());
    }

    #[test]
    fn test_request_id_generate() {
        let id = RequestId::generate();
        assert!(!id.as_str().is_empty());
        // Hex-encoded u128: up to 32 chars (leading zeros may be omitted)
        assert!(id.as_str().len() <= 32);
        // Must be valid hex
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_request_id_unique() {
        let id1 = RequestId::generate();
        let id2 = RequestId::generate();
        assert_ne!(id1.as_str(), id2.as_str());
    }

    #[test]
    fn test_provider_id_new() {
        let id = ProviderId::new("openai");
        assert_eq!(id.as_str(), "openai");
    }

    #[test]
    fn test_provider_id_display() {
        let id = ProviderId::new("anthropic");
        assert_eq!(format!("{id}"), "anthropic");
    }

    #[test]
    fn test_context_window_new() {
        let cw = ContextWindow::new(1000);
        assert_eq!(cw.as_u64(), 1000);
    }

    #[test]
    fn test_context_window_display() {
        assert_eq!(format!("{}", ContextWindow::new(500)), "500");
        assert_eq!(format!("{}", ContextWindow::new(1500)), "1K");
        assert_eq!(format!("{}", ContextWindow::new(1_500_000)), "1M");
        assert_eq!(format!("{}", ContextWindow::new(0)), "0");
    }

    #[test]
    fn test_finish_reason_from_upstream() {
        assert_eq!(FinishReason::from_upstream("stop"), FinishReason::Stop);
        assert_eq!(
            FinishReason::from_upstream("tool_use"),
            FinishReason::ToolCalls
        );
        assert_eq!(
            FinishReason::from_upstream("tool-calls"),
            FinishReason::ToolCalls
        );
        assert_eq!(
            FinishReason::from_upstream("tool_calls"),
            FinishReason::ToolCalls
        );
        assert_eq!(FinishReason::from_upstream("length"), FinishReason::Length);
        assert_eq!(
            FinishReason::from_upstream("max_tokens"),
            FinishReason::Length
        );
        assert_eq!(FinishReason::from_upstream("unknown"), FinishReason::Stop);
        assert_eq!(FinishReason::from_upstream(""), FinishReason::Stop);
    }

    #[test]
    fn test_finish_reason_display() {
        assert_eq!(format!("{}", FinishReason::Stop), "stop");
        assert_eq!(format!("{}", FinishReason::ToolCalls), "tool_calls");
        assert_eq!(format!("{}", FinishReason::Length), "length");
    }

    #[test]
    fn test_model_meta_creation() {
        let meta = ModelMeta {
            name: "GPT-4".into(),
            reasoning: true,
            efforts: vec![Effort::Low, Effort::High],
            context_window: ContextWindow::new(8192),
            provider: ProviderId::new("openai"),
        };
        assert_eq!(meta.name, "GPT-4");
        assert!(meta.reasoning);
        assert_eq!(meta.efforts.len(), 2);
        assert_eq!(meta.context_window.as_u64(), 8192);
        assert_eq!(meta.provider.as_str(), "openai");
    }
}

/// Model metadata — parsed from CLI bundled models.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMeta {
    /// Human-readable model name.
    pub name: String,
    /// Whether the model supports reasoning / chain-of-thought.
    pub reasoning: bool,
    /// Supported reasoning effort levels.
    pub efforts: Vec<Effort>,
    /// Context window size in tokens.
    pub context_window: ContextWindow,
    /// Provider that offers this model.
    pub provider: ProviderId,
}

/// Finish reason — exhaustive enum matching OpenAI spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// The model finished normally.
    Stop,
    /// The model requested one or more tool calls.
    ToolCalls,
    /// The output was truncated by max_tokens.
    Length,
}

impl FinishReason {
    /// Map upstream raw finish reason to our enum.
    pub fn from_upstream(raw: &str) -> Self {
        match raw.to_ascii_lowercase().as_str() {
            "tool_use" | "tool-calls" | "tool_calls" => Self::ToolCalls,
            "length" | "max_tokens" => Self::Length,
            _ => Self::Stop,
        }
    }
}

impl fmt::Display for FinishReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stop => write!(f, "stop"),
            Self::ToolCalls => write!(f, "tool_calls"),
            Self::Length => write!(f, "length"),
        }
    }
}
