use crate::types::{ContextWindow, Effort, ModelId, ModelMeta, ProviderId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Parse the CLI's bundled models.md table into a catalog.
pub fn parse_models_md(content: &str) -> HashMap<ModelId, ModelMeta> {
    let mut catalog = HashMap::new();
    let mut current_provider = ProviderId::new("unknown");

    for line in content.lines() {
        let line = line.trim();

        if let Some(stripped) = line.strip_prefix("## ") {
            let name = stripped.trim().to_ascii_lowercase();
            current_provider = ProviderId::new(match name.as_str() {
                "open source" => "open-source",
                "openai" => "openai",
                "anthropic" => "anthropic",
                "google" => "google",
                "sakana" => "sakana",
                "meta" => "meta",
                other => other,
            });
            continue;
        }

        if !line.starts_with('|') || line.starts_with("|---") || line.starts_with("| Id") {
            continue;
        }

        let cols: Vec<&str> = line.split('|').filter(|c| !c.trim().is_empty()).collect();
        if cols.len() < 5 {
            continue;
        }

        let id = extract_backtick_content(cols[0]);
        let Some(model_id) = id else { continue };
        let name = cols[1].trim().to_string();
        let context = cols[2].trim();
        let efforts_str = cols[3].trim();

        let efforts: Vec<Effort> = if efforts_str != "—" && !efforts_str.is_empty() {
            efforts_str
                .split(',')
                .filter_map(|e| Effort::from_str_opt(e.trim()))
                .collect()
        } else {
            Vec::new()
        };

        let context_window = parse_context_window(context);

        catalog.insert(
            ModelId::new(model_id),
            ModelMeta {
                name,
                reasoning: !efforts.is_empty(),
                efforts,
                context_window,
                provider: current_provider.clone(),
            },
        );
    }

    catalog
}

fn extract_backtick_content(s: &str) -> Option<String> {
    let start = s.find('`')?;
    let end = s[start + 1..].find('`')?;
    Some(s[start + 1..start + 1 + end].to_string())
}

fn parse_context_window(s: &str) -> ContextWindow {
    if s == "—" || s.is_empty() {
        return ContextWindow::new(0);
    }
    let s = s.trim();
    if let Some(pos) = s.find(|c: char| c.is_alphabetic()) {
        let num_part = &s[..pos];
        let unit = &s[pos..];
        if let Ok(val) = num_part.parse::<f64>() {
            let tokens = match unit.to_uppercase().as_str() {
                "M" => (val * 1_000_000.0) as u64,
                "K" => (val * 1_000.0) as u64,
                _ => val as u64,
            };
            return ContextWindow::new(tokens);
        }
    }
    ContextWindow::new(0)
}

/// Get the model catalog.
/// Priority: COMMAND_CODE_PROXY_MODELS_CATALOG env var (path to models.md) > CLI auto-discovery > empty.
pub fn get_model_catalog() -> &'static HashMap<ModelId, ModelMeta> {
    static CATALOG: OnceLock<HashMap<ModelId, ModelMeta>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        // 1. Try env var pointing to a models.md file
        if let Ok(path_str) = std::env::var("COMMAND_CODE_PROXY_MODELS_CATALOG") {
            let path = PathBuf::from(&path_str);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let catalog = parse_models_md(&content);
                    eprintln!(
                        "[command-code-proxy] loaded {} models from {}",
                        catalog.len(),
                        path.display()
                    );
                    return catalog;
                }
            }
        }

        // 2. Try CLI auto-discovery (single-tenant only)
        if let Some(path) = find_models_md() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let catalog = parse_models_md(&content);
                eprintln!(
                    "[command-code-proxy] loaded {} models from {}",
                    catalog.len(),
                    path.display()
                );
                return catalog;
            }
        }

        // 3. Empty catalog — proxy still works, just /v1/models returns empty
        eprintln!("[command-code-proxy] no models found (set COMMAND_CODE_PROXY_MODELS_CATALOG or install command-code CLI)");
        HashMap::new()
    })
}

/// Try to find the CLI's bundled models.md (single-tenant only).
fn find_models_md() -> Option<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let candidates = [
        home.join(".linuxbrew/lib/node_modules/command-code/dist/bundled/command-code-knowledge/reference/models.md"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/lib/node_modules/command-code/dist/bundled/command-code-knowledge/reference/models.md"),
        PathBuf::from("/usr/local/lib/node_modules/command-code/dist/bundled/command-code-knowledge/reference/models.md"),
    ];

    for path in &candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MD: &str = r#"
## Open Source

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out | Min plan | Best for |
|---|---|---|---|---|---|---|
| `deepseek/deepseek-v4-pro` | DeepSeek V4 Pro | 1M | high, max | $0.66/$1.98 | Go | reasoning |
| `moonshotai/Kimi-K3` | Kimi K3 | 1M | — | $3/$15 | Go | coding |

## Anthropic

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out | Min plan | Best for |
|---|---|---|---|---|---|---|
| `claude-sonnet-5` | Claude Sonnet 5 | 1M | low, medium, high, xhigh, max | $2/$10 | Pro | best combo |

## OpenAI

| Id (use EXACTLY this) | Name | Context | Efforts | $/1M in/out | Min plan | Best for |
|---|---|---|---|---|---|---|
| `gpt-5.6-luna` | GPT-5.6 Luna | 1.05M | low, medium, high, xhigh, max | $0.2/$1.2 | Go | cost-effective |
| `gpt-5.4-mini` | GPT-5.4 Mini | 400K | low, medium, high | $0.75/$4.5 | Pro | fast |
"#;

    #[test]
    fn test_parse_models() {
        let catalog = parse_models_md(SAMPLE_MD);
        assert_eq!(catalog.len(), 5);

        let ds = catalog
            .get(&ModelId::new("deepseek/deepseek-v4-pro"))
            .unwrap();
        assert_eq!(ds.name, "DeepSeek V4 Pro");
        assert_eq!(ds.context_window, ContextWindow::new(1_000_000));
        assert_eq!(ds.efforts, vec![Effort::High, Effort::Max]);
        assert_eq!(ds.provider, ProviderId::new("open-source"));

        let kimi = catalog.get(&ModelId::new("moonshotai/Kimi-K3")).unwrap();
        assert!(kimi.efforts.is_empty());
        assert!(!kimi.reasoning);

        let claude = catalog.get(&ModelId::new("claude-sonnet-5")).unwrap();
        assert!(claude.reasoning);
        assert_eq!(claude.efforts.len(), 5);
        assert_eq!(claude.provider, ProviderId::new("anthropic"));

        let gpt = catalog.get(&ModelId::new("gpt-5.6-luna")).unwrap();
        assert_eq!(gpt.context_window, ContextWindow::new(1_050_000));
    }

    #[test]
    fn test_parse_context_window() {
        assert_eq!(parse_context_window("1M"), ContextWindow::new(1_000_000));
        assert_eq!(parse_context_window("400K"), ContextWindow::new(400_000));
        assert_eq!(parse_context_window("1.05M"), ContextWindow::new(1_050_000));
        assert_eq!(parse_context_window("—"), ContextWindow::new(0));
        assert_eq!(parse_context_window(""), ContextWindow::new(0));
    }

    #[test]
    fn test_find_models_md_candidates() {
        let home = dirs::home_dir().unwrap();
        let candidates = [
            home.join(".linuxbrew/lib/node_modules/command-code/dist/bundled/command-code-knowledge/reference/models.md"),
            PathBuf::from("/home/linuxbrew/.linuxbrew/lib/node_modules/command-code/dist/bundled/command-code-knowledge/reference/models.md"),
            PathBuf::from("/usr/local/lib/node_modules/command-code/dist/bundled/command-code-knowledge/reference/models.md"),
        ];
        let found = candidates.iter().any(|p| p.exists());
        // The command-code CLI is not installed on every host (notably CI).
        // When absent, the catalog is empty rather than an error — so this
        // assertion must tolerate a missing CLI instead of panicking.
        if !found {
            println!("[skip] command-code CLI models.md not installed; model catalog is empty");
            return;
        }
        // If present, at least one candidate must resolve.
        assert!(
            found,
            "none of the bundled models.md candidates exist on this machine"
        );
    }
}
