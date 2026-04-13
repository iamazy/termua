use std::{
    collections::VecDeque,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::Context;

pub(crate) struct CommandHistory {
    path: PathBuf,
    max_entries: usize,
    entries: Mutex<VecDeque<String>>,
}

impl CommandHistory {
    pub(crate) fn load_default() -> anyhow::Result<Self> {
        let path = command_history_path();
        Self::load_from_path(path, 2000)
    }

    fn load_from_path(path: PathBuf, max_entries: usize) -> anyhow::Result<Self> {
        let entries = load_lines(&path, max_entries).with_context(|| format!("read {path:?}"))?;
        Ok(Self {
            path,
            max_entries,
            entries: Mutex::new(entries),
        })
    }

    fn append(&self, command: &str) {
        let mut command = command.trim().to_string();
        if command.is_empty() {
            return;
        }
        command = command.replace(['\n', '\r'], " ");

        let mut entries = self.entries.lock().unwrap();
        if entries.back().is_some_and(|v| v == &command) {
            return;
        }

        entries.push_back(command.clone());

        let mut needs_compact = false;
        while entries.len() > self.max_entries.max(1) {
            entries.pop_front();
            needs_compact = true;
        }

        if let Err(err) = ensure_parent_dir(&self.path) {
            log::warn!("termua: failed to create command history dir: {err:#}");
            return;
        }

        if needs_compact {
            let snapshot = entries.iter().cloned().collect::<Vec<_>>();
            drop(entries);
            if let Err(err) = write_compacted(&self.path, &snapshot) {
                log::warn!("termua: failed to compact command history: {err:#}");
            }
            return;
        }

        if let Err(err) = append_line(&self.path, &command) {
            log::warn!("termua: failed to append command history: {err:#}");
        }
    }
}

impl gpui_term::SuggestionHistoryProvider for CommandHistory {
    fn seed(&self) -> Vec<String> {
        self.entries.lock().unwrap().iter().cloned().collect()
    }

    fn append(&self, command: &str) {
        self.append(command);
    }
}

fn command_history_path() -> PathBuf {
    crate::settings::settings_dir_path().join("command_history.txt")
}

fn ensure_parent_dir(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {parent:?}"))?;
    }
    Ok(())
}

fn append_line(path: &Path, line: &str) -> anyhow::Result<()> {
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {path:?}"))?;
    writeln!(f, "{line}").with_context(|| format!("write {path:?}"))?;
    Ok(())
}

fn write_compacted(path: &Path, lines: &[String]) -> anyhow::Result<()> {
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    crate::atomic_write::write_string(path, &out).with_context(|| format!("write {path:?}"))?;
    Ok(())
}

fn load_lines(path: &Path, max_entries: usize) -> anyhow::Result<VecDeque<String>> {
    let contents = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).with_context(|| format!("read {path:?}")),
    };

    let mut out = VecDeque::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if out.back().is_some_and(|v| v == line) {
            continue;
        }
        out.push_back(line.to_string());
        while out.len() > max_entries.max(1) {
            out.pop_front();
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use gpui_term::SuggestionHistoryProvider as _;

    use super::*;

    #[test]
    fn persists_across_reload() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "termua-command-history-test-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let settings_path = dir.join("termua").join("settings.json");
        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let _guard = crate::settings::override_settings_json_path(settings_path);

        let history = CommandHistory::load_default().unwrap();
        history.append("ls --all");
        history.append("git status");

        let history2 = CommandHistory::load_default().unwrap();
        let seed = history2.seed();
        assert!(seed.contains(&"ls --all".to_string()));
        assert!(seed.contains(&"git status".to_string()));
    }
}
