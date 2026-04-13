use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default)]
pub(crate) struct StaticSuggestionsDb {
    by_command: BTreeMap<String, Vec<SuggestionsExample>>,
}

impl StaticSuggestionsDb {
    pub(crate) fn load_default() -> anyhow::Result<Self> {
        Self::load_from_dir(&suggestions_dir_path())
    }

    pub(crate) fn load_from_dir(dir: &Path) -> anyhow::Result<Self> {
        let mut by_command: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

        let entries = match fs::read_dir(dir) {
            Ok(v) => v,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => return Err(err).with_context(|| format!("read_dir {dir:?}")),
        };

        let mut json_files = Vec::<PathBuf>::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("read_dir entry in {dir:?}"))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                json_files.push(path);
            }
        }
        json_files.sort();

        for path in json_files {
            match load_one_json(&path) {
                Ok(map) => merge_suggestions(&mut by_command, map),
                Err(err) => {
                    log::warn!("termua: failed to load static suggestions {path:?}: {err:#}")
                }
            }
        }

        let by_command = by_command
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.into_iter()
                        .map(|(cmd, desc)| SuggestionsExample { cmd, desc })
                        .collect(),
                )
            })
            .collect::<BTreeMap<_, _>>();

        Ok(Self { by_command })
    }
}

impl gpui_term::SuggestionStaticProvider for StaticSuggestionsDb {
    fn for_each_candidate(&self, first_word: &str, f: &mut dyn FnMut(&str, Option<&str>)) {
        let first_word = first_word.trim();
        if first_word.is_empty() {
            return;
        }

        // Avoid scanning huge datasets for 1-character prefixes.
        if first_word.chars().count() <= 1 {
            if let Some(hints) = self.by_command.get(first_word) {
                for hint in hints {
                    f(
                        &hint.cmd,
                        (!hint.desc.trim().is_empty()).then_some(hint.desc.trim()),
                    );
                }
            }
            return;
        }

        let end = format!("{first_word}\u{10FFFF}");
        let mut yielded = 0usize;

        for (_cmd, hints) in self.by_command.range(first_word.to_string()..=end) {
            for hint in hints {
                f(
                    &hint.cmd,
                    (!hint.desc.trim().is_empty()).then_some(hint.desc.trim()),
                );
                yielded += 1;
                if yielded >= 500 {
                    return;
                }
            }
        }
    }
}

pub(crate) fn suggestions_dir_path() -> PathBuf {
    crate::settings::settings_dir_path().join("suggestions.d")
}

fn load_one_json(path: &Path) -> anyhow::Result<BTreeMap<String, Vec<SuggestionsExample>>> {
    let contents = fs::read_to_string(path).with_context(|| format!("read {path:?}"))?;

    if let Ok(v) = serde_json::from_str::<SuggestionsFile>(&contents) {
        let mut out = BTreeMap::<String, Vec<SuggestionsExample>>::new();
        for (cmd, entry) in v.commands {
            let mut candidates = Vec::<SuggestionsExample>::new();
            for ex in entry.examples {
                let cmd = ex.cmd.trim().to_string();
                if cmd.is_empty() {
                    continue;
                }
                candidates.push(SuggestionsExample {
                    cmd,
                    desc: ex.desc.trim().to_string(),
                });
            }
            out.insert(cmd, candidates);
        }
        return Ok(out);
    }

    let legacy: BTreeMap<String, Vec<String>> =
        serde_json::from_str(&contents).with_context(|| format!("parse {path:?}"))?;
    let mut out = BTreeMap::<String, Vec<SuggestionsExample>>::new();
    for (cmd, candidates) in legacy {
        let candidates = candidates
            .into_iter()
            .map(|c| SuggestionsExample {
                cmd: c,
                desc: String::new(),
            })
            .collect();
        out.insert(cmd, candidates);
    }
    Ok(out)
}

fn merge_suggestions(
    dst: &mut BTreeMap<String, BTreeMap<String, String>>,
    src: BTreeMap<String, Vec<SuggestionsExample>>,
) {
    for (command, hints) in src {
        let command = command.trim().to_string();
        if command.is_empty() {
            continue;
        }

        let slot = dst.entry(command.clone()).or_default();
        for hint in hints {
            let cmd = hint.cmd.trim().to_string();
            if cmd.is_empty() {
                continue;
            }

            // Keep data consistent with the append-only engine expectations.
            if !cmd.starts_with(&command) {
                continue;
            }

            let desc = hint.desc.trim().to_string();
            slot.entry(cmd)
                .and_modify(|existing| {
                    if existing.is_empty() && !desc.is_empty() {
                        *existing = desc.clone();
                    } else if desc.len() > existing.len() && !desc.is_empty() {
                        *existing = desc.clone();
                    }
                })
                .or_insert(desc);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SuggestionsFile {
    commands: BTreeMap<String, SuggestionsCommand>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SuggestionsCommand {
    examples: Vec<SuggestionsExample>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SuggestionsExample {
    cmd: String,
    desc: String,
}

#[cfg(test)]
mod tests {
    use gpui_term::SuggestionStaticProvider as _;

    use super::*;

    #[test]
    fn loads_suggestions_from_suggestions_dir() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "termua-static-suggestions-test-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let settings_path = dir.join("termua").join("settings.json");
        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let _guard = crate::settings::override_settings_json_path(settings_path);

        let suggestions_dir = suggestions_dir_path();
        std::fs::create_dir_all(&suggestions_dir).unwrap();
        std::fs::write(
            suggestions_dir.join("custom.json"),
            r#"{ "ls": ["ls -al", "ls -lah"] }"#,
        )
        .unwrap();
        std::fs::write(
            suggestions_dir.join("custom-v2.json"),
            r#"
{
  "commands": {
    "ls": {
      "examples": [
        { "cmd": "ls --all", "desc": "list all" }
      ]
    }
  }
}
"#,
        )
        .unwrap();

        let db = StaticSuggestionsDb::load_default().unwrap();

        let mut out = Vec::<(String, Option<String>)>::new();
        db.for_each_candidate("ls", &mut |cmd, desc| {
            out.push((cmd.to_string(), desc.map(|s| s.to_string())));
        });

        assert!(out.iter().any(|(cmd, _)| cmd == "ls -al"));
        assert!(out.iter().any(|(cmd, _)| cmd == "ls -lah"));
        assert!(
            out.iter()
                .any(|(cmd, desc)| cmd == "ls --all" && desc.as_deref() == Some("list all"))
        );
    }
}
