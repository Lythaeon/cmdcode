use cmdcode_core::model_catalog::get_model_catalog;

pub fn run() {
    let catalog = get_model_catalog();

    if catalog.is_empty() {
        eprintln!("no models found.");
        eprintln!();
        eprintln!(
            "ensure the command-code CLI is installed, or set COMMAND_CODE_PROXY_MODELS_CATALOG"
        );
        return;
    }

    println!(
        "{:<40} {:<20} {:<10} {:<10}",
        "ID", "NAME", "CTX", "REASONING"
    );
    println!("{}", "-".repeat(80));

    let mut models: Vec<_> = catalog.iter().collect();
    models.sort_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));

    for (id, meta) in &models {
        let ctx = if meta.context_window.as_u64() > 0 {
            let tokens = meta.context_window.as_u64();
            if tokens >= 1_000_000 {
                format!("{}M", tokens / 1_000_000)
            } else if tokens >= 1_000 {
                format!("{}K", tokens / 1_000)
            } else {
                tokens.to_string()
            }
        } else {
            "-".into()
        };

        let reasoning = if meta.reasoning {
            let efforts: Vec<&str> = meta.efforts.iter().map(|e| e.as_str()).collect();
            efforts.join(", ")
        } else {
            "-".into()
        };

        println!(
            "{:<40} {:<20} {:<10} {}",
            id.as_ref(),
            meta.name,
            ctx,
            reasoning
        );
    }

    println!();
    println!("{} models total", catalog.len());
}
