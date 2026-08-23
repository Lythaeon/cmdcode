use cmdcode_core::accounts::AccountStore;
use inquire::{Confirm, MultiSelect, Password, Select};

mod login;

/// Resolve the account store, defaulting to a non-existent vault (empty).
fn store() -> AccountStore {
    AccountStore::default()
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

/// Display the interactive TUI: `cmdcode auth`.
pub fn run() {
    loop {
        let vault = match store().load() {
            Ok(v) => v,
            Err(e) => fail(&format!("failed to read accounts vault: {e}")),
        };

        if vault.is_empty() {
            println!("no accounts yet");
            match Confirm::new("Sign in to a Command Code account now?")
                .with_default(true)
                .prompt()
            {
                Ok(true) => add(),
                _ => return,
            }
            continue;
        }

        let active = vault.active_account().map(|a| a.display_name().to_string());
        let auto = vault.settings.auto_rotate;

        println!();
        println!("Accounts ({}):", vault.len());
        for acct in &vault.accounts {
            let is_active = vault.active.as_deref() == Some(&acct.id());
            let marker = if is_active { "*" } else { " " };
            let suffix = if is_active { "  (active)" } else { "" };
            println!(" {marker} {}{suffix}", acct.display_name());
        }
        println!();

        enum Menu {
            Use,
            Add,
            Logout,
            Rotate,
            Quit,
        }

        let items = [
            ("Switch active account".to_string(), Menu::Use),
            ("Sign in a new account".to_string(), Menu::Add),
            ("Log out account(s)".to_string(), Menu::Logout),
            (
                format!("Auto-rotate: {}", if auto { "ON" } else { "OFF" }),
                Menu::Rotate,
            ),
            ("Done".to_string(), Menu::Quit),
        ];

        let labels: Vec<String> = items.iter().map(|(l, _)| l.clone()).collect();
        let title = format!(
            "Select an action — active: {}",
            active.as_deref().unwrap_or("none")
        );
        let selection = Select::new(&title, labels)
            .with_help_message("↑/↓ to move · Enter to select · Esc to exit")
            .prompt();

        let chosen = match selection {
            Ok(chosen_label) => items
                .iter()
                .find(|(l, _)| l == &chosen_label)
                .map(|(_, m)| m),
            Err(_) => return,
        };

        match chosen {
            Some(Menu::Use) => use_account(),
            Some(Menu::Add) => add(),
            Some(Menu::Logout) => logout(),
            Some(Menu::Rotate) => toggle_auto_rotate(None),
            Some(Menu::Quit) | None => return,
        }
    }
}

/// Non-interactive: list all accounts in the vault.
pub fn list() {
    let vault = match store().load() {
        Ok(v) => v,
        Err(e) => fail(&format!("failed to read vault: {e}")),
    };
    if vault.is_empty() {
        println!("no accounts");
        return;
    }
    println!("Accounts ({}):", vault.len());
    for acct in &vault.accounts {
        let is_active = vault.active.as_deref() == Some(&acct.id());
        let marker = if is_active { "*" } else { " " };
        let suffix = if is_active { "  (active)" } else { "" };
        println!(" {marker} {}{suffix}", acct.display_name());
    }
}

pub fn use_account() {
    let mut vault = match store().load() {
        Ok(v) => v,
        Err(e) => fail(&format!("failed to read vault: {e}")),
    };
    if vault.is_empty() {
        println!("no accounts to switch");
        return;
    }
    let names: Vec<String> = vault
        .accounts
        .iter()
        .map(|a| a.display_name().to_string())
        .collect();
    let choice = match Select::new("Activate account:", names.clone()).prompt() {
        Ok(c) => c,
        Err(_) => return,
    };
    let idx = match names.iter().position(|n| n == &choice) {
        Some(i) => i,
        None => return,
    };
    let id = vault.accounts[idx].id();
    if vault.set_active(&id).is_ok() {
        if let Err(e) = store().save(&vault) {
            println!("failed to save vault: {e}");
        } else {
            println!("now using {choice}");
        }
    }
}

pub fn logout() {
    let mut vault = match store().load() {
        Ok(v) => v,
        Err(e) => fail(&format!("failed to read vault: {e}")),
    };
    if vault.is_empty() {
        println!("no accounts to log out");
        return;
    }
    let names: Vec<String> = vault
        .accounts
        .iter()
        .map(|a| a.display_name().to_string())
        .collect();
    let choices = match MultiSelect::new("Select account(s) to log out:", names.clone()).prompt() {
        Ok(c) => c,
        Err(_) => return,
    };
    if choices.is_empty() {
        return;
    }
    let ids: Vec<String> = choices
        .iter()
        .filter_map(|name| {
            vault
                .accounts
                .iter()
                .find(|a| a.display_name() == name)
                .map(|a| a.id())
        })
        .collect();
    let ids_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
    if vault.remove(&ids_refs).is_ok() {
        if let Err(e) = store().save(&vault) {
            println!("failed to save vault: {e}");
        } else {
            println!("logged out {} account(s)", choices.len());
        }
    }
}

pub fn toggle_auto_rotate(state: Option<&str>) {
    let mut vault = match store().load() {
        Ok(v) => v,
        Err(e) => fail(&format!("failed to read vault: {e}")),
    };
    let new_val = match state {
        Some("on") | Some("ON") | Some("true") | Some("1") => true,
        Some("off") | Some("OFF") | Some("false") | Some("0") => false,
        _ => {
            let current = if vault.settings.auto_rotate {
                "ON"
            } else {
                "OFF"
            };
            let ok = Confirm::new(&format!(
                "Current: auto-rotate {current}\nEnable auto-rotate when an account hits its limit / is rejected?"
            ))
            .with_default(!vault.settings.auto_rotate)
            .prompt();
            matches!(ok, Ok(true))
        }
    };
    vault.settings.auto_rotate = new_val;
    if let Err(e) = store().save(&vault) {
        println!("failed to save vault: {e}");
    } else {
        let now = if vault.settings.auto_rotate {
            "ON"
        } else {
            "OFF"
        };
        println!("auto-rotate is now {now}");
    }
}

pub fn add() {
    let (port, state, url) = match login::make_auth_url() {
        Ok(v) => v,
        Err(e) => {
            println!("{e}");
            return;
        }
    };

    println!("Complete login in your browser:");
    println!("If your browser doesn't open, go to this link:");
    println!("  {url}\n");

    let key = match Password::new("API key (or press Enter to wait for browser callback):")
        .without_confirmation()
        .with_display_mode(inquire::PasswordDisplayMode::Masked)
        .prompt()
    {
        Ok(k) => k.trim().to_string(),
        Err(_) => return,
    };

    if !key.is_empty() {
        let upstream_url = std::env::var("COMMAND_CODE_API_BASE")
            .unwrap_or_else(|_| "https://api.commandcode.ai".to_string());
        match login::login_with_api_key(&key, &upstream_url) {
            Ok(account) => add_account(account),
            Err(e) => println!("validation failed: {e}"),
        }
    } else {
        println!("Waiting for authorization…");
        match login::run_callback_server(port, &state) {
            Ok(account) => add_account(account),
            Err(e) => println!("callback failed: {e}"),
        }
    }
}

fn add_account(account: cmdcode_core::accounts::Account) {
    let name = account.display_name().to_string();
    let mut vault = match store().load() {
        Ok(v) => v,
        Err(e) => fail(&format!("failed to read vault: {e}")),
    };
    match vault.add(account) {
        Ok(()) => {
            if let Err(e) = store().save(&vault) {
                println!("failed to save vault: {e}");
            } else {
                println!("added {name}");
            }
        }
        Err(cmdcode_core::error::AuthError::AccountExists(_)) => {
            println!("{name} is already signed in");
        }
        Err(e) => println!("failed to add account: {e}"),
    }
}
