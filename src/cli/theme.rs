//! `sendito theme`: lists, shows and checks themes.

use crate::*;

pub(crate) fn theme_registry(collection: Option<&Path>) -> sendito_ui::theme::Registry {
    let root = find_collection(collection);
    let dirs = sendito_store::themes::dirs(sendito_store::config::global_path().as_deref(), &root);
    let (files, warnings) = sendito_store::themes::discover(&dirs);
    sendito_ui::theme::Registry::from_files(files, warnings)
}

pub(crate) fn theme(cmd: ThemeCmd) -> ExitCode {
    match cmd {
        ThemeCmd::List { collection } => {
            let reg = theme_registry(collection.as_deref());
            for name in reg.names() {
                let path = reg
                    .path(&name)
                    .map(|p| format!("  {}", p.display()))
                    .unwrap_or_else(|| "  (built in)".into());
                println!("{name:<16}{path}");
                for n in reg.notes(&name) {
                    println!("{:<16}  ! {n}", "");
                }
            }
            for w in &reg.warnings {
                eprintln!("sendito: {w}");
            }
            ExitCode::SUCCESS
        }
        ThemeCmd::Print { name, collection } => {
            let reg = theme_registry(collection.as_deref());
            if !reg.is_known(&name) {
                eprintln!(
                    "sendito: no theme named \"{name}\" (available: {})",
                    reg.names().join(", ")
                );
                return ExitCode::from(3);
            }
            print!("{}", reg.theme_full(&name).to_toml());
            ExitCode::SUCCESS
        }
    }
}
