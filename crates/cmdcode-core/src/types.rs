use serde::{Deserialize, Serialize};
use std::fmt;

/// Newtype for model identifiers — never bare strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(String);

impl ModelId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

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
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
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
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Request identifier newtype.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    pub fn generate() -> Self {
        let id = uuid::Uuid::new_v4();
        Self(format!("{:x}", id.as_u128()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Provider identifier newtype.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

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
    pub fn new(tokens: u64) -> Self {
        Self(tokens)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// Model metadata — parsed from CLI bundled models.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMeta {
    pub name: String,
    pub reasoning: bool,
    pub efforts: Vec<Effort>,
    pub context_window: ContextWindow,
    pub provider: ProviderId,
}

/// Finish reason — exhaustive enum matching OpenAI spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
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
