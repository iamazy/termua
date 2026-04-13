use std::{env, fs::OpenOptions, path::PathBuf};

use crate::settings::SettingsFile;

pub(crate) fn init_logging(settings: &SettingsFile) {
    let mut builder = env_logger::Builder::new();

    // Precedence: env filter string > settings level.
    if let Ok(filters) = env::var("TERMUA_LOG").or_else(|_| env::var("RUST_LOG")) {
        builder.parse_filters(&filters);
    } else {
        builder.filter_level(settings.logging.level.to_level_filter());
    }

    if let Some(path) = settings
        .logging
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let resolved = resolve_log_path(path);
        if let Some(parent) = resolved.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match OpenOptions::new().create(true).append(true).open(&resolved) {
            Ok(file) => {
                builder.target(env_logger::Target::Pipe(Box::new(file)));
            }
            Err(err) => {
                eprintln!("failed to open log file {resolved:?}: {err:#}");
            }
        }
    }

    builder.init();
}

fn resolve_log_path(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        return p;
    }

    crate::settings::settings_dir_path().join(p)
}
