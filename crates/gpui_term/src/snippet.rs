use std::ops::Range;

use crate::GridPoint;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TabStop {
    pub(crate) index: u32,
    pub(crate) range_chars: Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnippetSuffix {
    pub(crate) rendered: String,
    pub(crate) tabstops: Vec<TabStop>,
}

pub(crate) fn parse_snippet_suffix(input: &str) -> Option<SnippetSuffix> {
    #[derive(Clone)]
    struct ParsedStop {
        index: u32,
        range_chars: Range<usize>,
        appearance: usize,
    }

    let mut rendered = String::with_capacity(input.len());
    let mut rendered_len_chars = 0usize;
    let mut stops = Vec::<ParsedStop>::new();
    let mut appearance = 0usize;

    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let mut j = i + 2;
            let digits_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }

            if j == digits_start {
                // Not a `${<digits>...}` placeholder; treat `$` literally.
                rendered.push('$');
                rendered_len_chars += 1;
                i += 1;
                continue;
            }

            let index: u32 = match input[digits_start..j].parse() {
                Ok(v) => v,
                Err(_) => {
                    rendered.push('$');
                    rendered_len_chars += 1;
                    i += 1;
                    continue;
                }
            };

            let (default_start, default_end, next_i) = match bytes.get(j) {
                Some(b'}') => (j, j, j + 1),
                Some(b':') => {
                    let start = j + 1;
                    let mut end = start;
                    while end < bytes.len() && bytes[end] != b'}' {
                        end += 1;
                    }
                    if end >= bytes.len() {
                        rendered.push('$');
                        rendered_len_chars += 1;
                        i += 1;
                        continue;
                    }
                    (start, end, end + 1)
                }
                _ => {
                    rendered.push('$');
                    rendered_len_chars += 1;
                    i += 1;
                    continue;
                }
            };

            let start_chars = rendered_len_chars;

            // `${0}` is treated as the final cursor position, not a placeholder with content.
            if index != 0 {
                let default_text = &input[default_start..default_end];
                rendered.push_str(default_text);
                rendered_len_chars += default_text.chars().count();
            }

            let end_chars = rendered_len_chars;
            stops.push(ParsedStop {
                index,
                range_chars: start_chars..end_chars,
                appearance,
            });
            appearance += 1;

            i = next_i;
            continue;
        }

        let ch = input[i..].chars().next().unwrap();
        rendered.push(ch);
        rendered_len_chars += 1;
        i += ch.len_utf8();
    }

    if stops.is_empty() {
        return None;
    }

    stops.sort_by(|a, b| {
        (a.index == 0)
            .cmp(&(b.index == 0))
            .then_with(|| a.index.cmp(&b.index))
            .then_with(|| a.appearance.cmp(&b.appearance))
    });

    let tabstops = stops
        .into_iter()
        .map(|s| TabStop {
            index: s.index,
            range_chars: s.range_chars,
        })
        .collect();

    Some(SnippetSuffix { rendered, tabstops })
}

fn byte_index_for_char(s: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }
    for (count, (idx, _)) in s.char_indices().enumerate() {
        if count == char_index {
            return idx;
        }
    }
    s.len()
}

fn byte_range_for_char_range(s: &str, range_chars: Range<usize>) -> Range<usize> {
    let start = byte_index_for_char(s, range_chars.start);
    let end = byte_index_for_char(s, range_chars.end);
    start..end
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnippetSession {
    pub(crate) inserted_len_chars: usize,
    pub(crate) start_point: GridPoint,
    pub(crate) rendered: String,
    pub(crate) tabstops: Vec<TabStop>,
    pub(crate) active: usize,
    pub(crate) cursor_offset_chars: usize,
    pub(crate) selected: bool,
    pub(crate) cursor_line_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SnippetJump {
    Noop,
    Move(isize),
    Exit(isize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SnippetJumpDir {
    Next,
    Prev,
}

impl SnippetSession {
    pub(crate) fn new(rendered: String, tabstops: Vec<TabStop>) -> Self {
        let inserted_len_chars = rendered.chars().count();
        Self {
            inserted_len_chars,
            start_point: GridPoint::new(0, 0),
            rendered,
            tabstops,
            active: 0,
            cursor_offset_chars: inserted_len_chars,
            selected: false,
            cursor_line_id: None,
        }
    }

    pub(crate) fn delete_active_placeholder(&mut self) -> usize {
        let Some(stop) = self.tabstops.get(self.active).cloned() else {
            return 0;
        };
        let start = stop.range_chars.start;
        let end = stop.range_chars.end;
        let len = end.saturating_sub(start);
        if len == 0 {
            return 0;
        }

        let delete_range_bytes = byte_range_for_char_range(&self.rendered, start..end);
        if delete_range_bytes.start < delete_range_bytes.end
            && delete_range_bytes.end <= self.rendered.len()
        {
            self.rendered.replace_range(delete_range_bytes.clone(), "");
        }

        for (i, t) in self.tabstops.iter_mut().enumerate() {
            if i == self.active {
                t.range_chars = start..start;
                continue;
            }

            // Shift tabstops that are to the right of the deleted placeholder in the *text*,
            // regardless of tabstop order.
            if t.range_chars.start >= end {
                t.range_chars = (t.range_chars.start - len)..(t.range_chars.end - len);
            }
        }
        self.inserted_len_chars = self.inserted_len_chars.saturating_sub(len);
        self.cursor_offset_chars = start;
        len
    }

    pub(crate) fn replace_active_placeholder(&mut self, inserted: &str) -> usize {
        let inserted_chars = inserted.chars().count();
        let deleted = self.delete_active_placeholder();
        if inserted_chars == 0 {
            return deleted;
        }

        let Some(active) = self.tabstops.get_mut(self.active) else {
            return deleted;
        };

        let start = active.range_chars.start;
        active.range_chars = start..(start + inserted_chars);
        for (i, t) in self.tabstops.iter_mut().enumerate() {
            if i == self.active {
                continue;
            }
            if t.range_chars.start >= start {
                t.range_chars =
                    (t.range_chars.start + inserted_chars)..(t.range_chars.end + inserted_chars);
            }
        }

        let insert_at = byte_index_for_char(&self.rendered, start);
        if insert_at <= self.rendered.len() {
            self.rendered.insert_str(insert_at, inserted);
        }

        self.inserted_len_chars += inserted_chars;
        self.cursor_offset_chars = start + inserted_chars;
        deleted
    }

    pub(crate) fn jump(&mut self, dir: SnippetJumpDir) -> SnippetJump {
        if self.tabstops.is_empty() || self.active >= self.tabstops.len() {
            return SnippetJump::Exit(0);
        }

        match dir {
            SnippetJumpDir::Prev => {
                if self.active == 0 {
                    return SnippetJump::Noop;
                }
                self.active -= 1;
            }
            SnippetJumpDir::Next => {
                if self.active + 1 >= self.tabstops.len() {
                    let delta =
                        self.inserted_len_chars as isize - self.cursor_offset_chars as isize;
                    self.cursor_offset_chars = self.inserted_len_chars;
                    return SnippetJump::Exit(delta);
                }
                self.active += 1;
            }
        }

        let target_end = self.tabstops[self.active].range_chars.end;
        let delta = target_end as isize - self.cursor_offset_chars as isize;
        self.cursor_offset_chars = target_end;
        self.selected = true;
        SnippetJump::Move(delta)
    }

    pub(crate) fn insert_into_active_placeholder(&mut self, inserted: &str) {
        let inserted_chars = inserted.chars().count();
        if inserted_chars == 0 {
            return;
        }

        let Some(active) = self.tabstops.get_mut(self.active) else {
            return;
        };

        if self.cursor_offset_chars != active.range_chars.end {
            return;
        }

        let old_end = active.range_chars.end;
        active.range_chars.end += inserted_chars;
        for (i, t) in self.tabstops.iter_mut().enumerate() {
            if i == self.active {
                continue;
            }
            if t.range_chars.start >= old_end {
                t.range_chars =
                    (t.range_chars.start + inserted_chars)..(t.range_chars.end + inserted_chars);
            }
        }

        let insert_at = byte_index_for_char(&self.rendered, old_end);
        if insert_at <= self.rendered.len() {
            self.rendered.insert_str(insert_at, inserted);
        }

        self.inserted_len_chars += inserted_chars;
        self.cursor_offset_chars += inserted_chars;
    }

    pub(crate) fn backspace_one_in_active_placeholder(&mut self) -> bool {
        let Some(active) = self.tabstops.get_mut(self.active) else {
            return false;
        };

        if self.cursor_offset_chars != active.range_chars.end {
            return false;
        }

        if active.range_chars.end <= active.range_chars.start {
            return false;
        }

        let old_end = active.range_chars.end;
        let delete_start = old_end.saturating_sub(1);
        let delete_range_bytes = byte_range_for_char_range(&self.rendered, delete_start..old_end);
        if delete_range_bytes.start < delete_range_bytes.end
            && delete_range_bytes.end <= self.rendered.len()
        {
            self.rendered.replace_range(delete_range_bytes, "");
        }
        active.range_chars.end -= 1;
        for (i, t) in self.tabstops.iter_mut().enumerate() {
            if i == self.active {
                continue;
            }
            if t.range_chars.start >= old_end {
                t.range_chars = (t.range_chars.start - 1)..(t.range_chars.end - 1);
            }
        }

        self.inserted_len_chars = self.inserted_len_chars.saturating_sub(1);
        self.cursor_offset_chars = self.cursor_offset_chars.saturating_sub(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range_for_substring_chars(haystack: &str, needle: &str) -> Range<usize> {
        let start_bytes = haystack
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in {haystack:?}"));
        let start_chars = haystack[..start_bytes].chars().count();
        let end_chars = start_chars + needle.chars().count();
        start_chars..end_chars
    }

    fn range_for_last_substring_chars(haystack: &str, needle: &str) -> Range<usize> {
        let start_bytes = haystack
            .rfind(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in {haystack:?}"));
        let start_chars = haystack[..start_bytes].chars().count();
        let end_chars = start_chars + needle.chars().count();
        start_chars..end_chars
    }

    #[test]
    fn parses_placeholders_and_renders_defaults() {
        let s = parse_snippet_suffix(" --message ${2:body} --subject ${1:sub}${0}").unwrap();
        assert_eq!(s.rendered, " --message body --subject sub");

        assert_eq!(s.tabstops.len(), 3);
        assert_eq!(s.tabstops[0].index, 1);
        assert_eq!(
            s.tabstops[0].range_chars,
            range_for_last_substring_chars(&s.rendered, "sub")
        );
        assert_eq!(s.tabstops[1].index, 2);
        assert_eq!(
            s.tabstops[1].range_chars,
            range_for_substring_chars(&s.rendered, "body")
        );
        assert_eq!(s.tabstops[2].index, 0);
        let end = s.rendered.chars().count();
        assert_eq!(s.tabstops[2].range_chars, end..end);
    }

    #[test]
    fn supports_empty_placeholders() {
        let s = parse_snippet_suffix("git commit -m ${1}${0}").unwrap();
        assert_eq!(s.rendered, "git commit -m ");
        assert_eq!(s.tabstops.len(), 2);
        assert_eq!(s.tabstops[0].index, 1);
        assert_eq!(
            s.tabstops[0].range_chars,
            s.rendered.chars().count()..s.rendered.chars().count()
        );
    }

    #[test]
    fn returns_none_when_no_valid_placeholders_present() {
        assert!(parse_snippet_suffix(" --help").is_none());
        assert!(parse_snippet_suffix("${x:not-a-tabstop}").is_none());
    }

    #[test]
    fn deleting_active_placeholder_shifts_following_tabstops_left() {
        let mut s = SnippetSession::new(
            "aXbYc".to_string(),
            vec![
                TabStop {
                    index: 1,
                    range_chars: 1..2,
                },
                TabStop {
                    index: 2,
                    range_chars: 3..4,
                },
            ],
        );
        s.active = 0;
        let deleted = s.delete_active_placeholder();
        assert_eq!(deleted, 1);
        assert_eq!(s.inserted_len_chars, 4);
        assert_eq!(s.tabstops[0].range_chars, 1..1);
        assert_eq!(s.tabstops[1].range_chars, 2..3);
    }

    #[test]
    fn deleting_active_placeholder_does_not_shift_tabstops_before_active_in_text() {
        // Active placeholder is later in text, but earlier in tab order (index).
        let mut s = SnippetSession::new(
            "0123456789abc".to_string(),
            vec![
                TabStop {
                    index: 1,
                    range_chars: 10..13,
                },
                TabStop {
                    index: 2,
                    range_chars: 2..4,
                },
            ],
        );
        s.active = 0;

        let deleted = s.delete_active_placeholder();
        assert_eq!(deleted, 3);

        // Placeholder `${2}` was before `${1}` in the text, so it should not move.
        assert_eq!(s.tabstops[1].range_chars, 2..4);
    }

    #[test]
    fn replacing_active_placeholder_updates_following_ranges() {
        let mut s = SnippetSession::new(
            "aXbYc".to_string(),
            vec![
                TabStop {
                    index: 1,
                    range_chars: 1..2,
                },
                TabStop {
                    index: 2,
                    range_chars: 3..4,
                },
            ],
        );
        s.active = 0;
        s.cursor_offset_chars = 2;

        let deleted = s.replace_active_placeholder("ZZZ");
        assert_eq!(deleted, 1);
        assert_eq!(s.inserted_len_chars, 7);
        assert_eq!(s.tabstops[0].range_chars, 1..4);
        assert_eq!(s.tabstops[1].range_chars, 5..6);
        assert_eq!(s.cursor_offset_chars, 4);
    }

    #[test]
    fn jump_next_and_exit_move_cursor_by_expected_delta() {
        let mut s = SnippetSession::new(
            "0123456789".to_string(),
            vec![
                TabStop {
                    index: 1,
                    range_chars: 2..4,
                },
                TabStop {
                    index: 2,
                    range_chars: 6..7,
                },
                TabStop {
                    index: 0,
                    range_chars: 10..10,
                },
            ],
        );
        s.active = 0;
        s.cursor_offset_chars = 4;

        assert_eq!(s.jump(SnippetJumpDir::Next), SnippetJump::Move(3));
        assert_eq!(s.cursor_offset_chars, 7);
        assert_eq!(s.active, 1);

        assert_eq!(s.jump(SnippetJumpDir::Next), SnippetJump::Move(3));
        assert_eq!(s.cursor_offset_chars, 10);
        assert_eq!(s.active, 2);

        assert_eq!(s.jump(SnippetJumpDir::Next), SnippetJump::Exit(0));
        assert_eq!(s.cursor_offset_chars, 10);
    }

    #[test]
    fn insert_and_backspace_update_ranges_in_place() {
        let mut s = SnippetSession::new(
            "aXbY".to_string(),
            vec![
                TabStop {
                    index: 1,
                    range_chars: 1..2,
                },
                TabStop {
                    index: 2,
                    range_chars: 3..4,
                },
            ],
        );
        s.active = 0;
        s.cursor_offset_chars = 2;

        s.insert_into_active_placeholder("zz");
        assert_eq!(s.inserted_len_chars, 6);
        assert_eq!(s.tabstops[0].range_chars, 1..4);
        assert_eq!(s.tabstops[1].range_chars, 5..6);
        assert_eq!(s.cursor_offset_chars, 4);

        assert!(s.backspace_one_in_active_placeholder());
        assert_eq!(s.inserted_len_chars, 5);
        assert_eq!(s.tabstops[0].range_chars, 1..3);
        assert_eq!(s.tabstops[1].range_chars, 4..5);
        assert_eq!(s.cursor_offset_chars, 3);
    }
}
