// TODO: refactor this heresy

use crate::frecency::FrecencyTracker;
use crate::keyword;
use crate::lsp_item::LspItem;
use frizbee::Options;
use mlua::prelude::*;
use mlua::FromLua;
use mlua::Lua;
use std::collections::HashMap;
use std::collections::HashSet;

#[derive(Clone, Hash)]
pub struct FuzzyOptions {
    match_suffix: bool,
    max_typos: u16,
    use_frecency: bool,
    use_proximity: bool,
    nearby_words: Option<Vec<String>>,
    snippet_score_offset: i32,
}

impl FromLua for FuzzyOptions {
    fn from_lua(value: LuaValue, _lua: &'_ Lua) -> LuaResult<Self> {
        if let Some(tab) = value.as_table() {
            let match_suffix: bool = tab.get("match_suffix").unwrap_or_default();
            let max_typos: u16 = tab.get("max_typos").unwrap_or_default();
            let use_frecency: bool = tab.get("use_frecency").unwrap_or_default();
            let use_proximity: bool = tab.get("use_proximity").unwrap_or_default();
            let nearby_words: Option<Vec<String>> = tab.get("nearby_words").ok();
            let snippet_score_offset: i32 = tab.get("snippet_score_offset").unwrap_or_default();

            Ok(FuzzyOptions {
                match_suffix,
                max_typos,
                use_frecency,
                use_proximity,
                nearby_words,
                snippet_score_offset,
            })
        } else {
            Err(mlua::Error::FromLuaConversionError {
                from: "LuaValue",
                to: "FuzzyOptions".to_string(),
                message: None,
            })
        }
    }
}

fn group_by_needle(
    line: &str,
    cursor_col: usize,
    haystack: &[String],
    match_suffix: bool,
) -> HashMap<String, Vec<(usize, String)>> {
    let mut items_by_needle: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    for (idx, item_text) in haystack.iter().enumerate() {
        let needle = keyword::guess_keyword_from_item(item_text, line, cursor_col, match_suffix);
        let entry = items_by_needle.entry(needle).or_default();
        entry.push((idx, item_text.to_string()));
    }
    items_by_needle
}

pub fn fuzzy(
    line: &str,
    cursor_col: usize,
    haystack: &[LspItem],
    frecency: &FrecencyTracker,
    opts: FuzzyOptions,
) -> (Vec<i32>, Vec<u32>, Vec<bool>) {
    let haystack_labels = haystack
        .iter()
        .map(|s| {
            let mut text = s.filter_text.clone().unwrap_or(s.label.clone());
            if text.len() > 1024 {
                let mut current_len = 0;
                for c in text.chars() {
                    // Check if adding this character would exceed 1024 bytes
                    let char_byte_len = c.len_utf8();
                    if current_len + char_byte_len > 1024 {
                        break;
                    }
                    current_len += char_byte_len;
                }
                // Truncate to the valid byte boundary
                text.truncate(current_len);
                text
            } else {
                text
            }
        })
        .collect::<Vec<_>>();
    let options = frizbee::Options {
        max_typos: Some(opts.max_typos),
        ..Default::default()
    };

    // Items may have different fuzzy matching ranges, so we split them up by needle
    let mut matches = group_by_needle(line, cursor_col, &haystack_labels, opts.match_suffix)
        .into_iter()
        // Match on each needle and combine
        .flat_map(|(needle, haystack)| {
            let mut matches = frizbee::match_list(
                &needle,
                &haystack
                    .iter()
                    .map(|(_, str)| str.as_str())
                    .collect::<Vec<_>>(),
                options,
            );
            for mtch in matches.iter_mut() {
                mtch.index_in_haystack = haystack[mtch.index_in_haystack].0;
            }
            matches
        })
        .collect::<Vec<_>>();

    matches.sort_by_key(|mtch| mtch.index_in_haystack);

    // Get the score for each match, adding score_offset, frecency and proximity bonus
    let nearby_words: HashSet<String> = HashSet::from_iter(opts.nearby_words.unwrap_or_default());
    let match_scores = matches
        .iter()
        .map(|mtch| {
            let frecency_score = if opts.use_frecency {
                frecency.get_score(&haystack[mtch.index_in_haystack]) as i32
            } else {
                0
            };
            let nearby_words_score = if opts.use_proximity {
                nearby_words
                    .get(&haystack_labels[mtch.index_in_haystack])
                    .map(|_| 2)
                    .unwrap_or(0)
            } else {
                0
            };
            let mut score_offset = haystack[mtch.index_in_haystack].score_offset;
            // 15 = snippet
            // TODO: use an enum for the kind
            if haystack[mtch.index_in_haystack].kind == 15 {
                score_offset += opts.snippet_score_offset;
            }

            (mtch.score as i32) + frecency_score + nearby_words_score + score_offset
        })
        .collect::<Vec<_>>();

    // Return scores and indices
    (
        match_scores,
        matches
            .iter()
            .map(|mtch| mtch.index_in_haystack as u32)
            .collect::<Vec<_>>(),
        matches.iter().map(|mtch| mtch.exact).collect::<Vec<_>>(),
    )
}

pub fn fuzzy_matched_indices(
    line: &str,
    cursor_col: usize,
    haystack: &[String],
    match_suffix: bool,
) -> Vec<Vec<usize>> {
    let mut matches = group_by_needle(line, cursor_col, haystack, match_suffix)
        .into_iter()
        .flat_map(|(needle, haystack)| {
            let mut matches = frizbee::match_list(
                &needle,
                &haystack
                    .iter()
                    .map(|(_, str)| str.as_str())
                    .collect::<Vec<_>>(),
                Options {
                    matched_indices: true,
                    stable_sort: false,
                    ..Default::default()
                },
            );
            matches.sort_by_key(|mtch| mtch.index_in_haystack);

            matches
                .into_iter()
                .enumerate()
                .map(|(idx, match_)| (haystack[idx].0, match_.indices.unwrap_or_default()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|mtch| mtch.0);

    matches
        .into_iter()
        .map(|(_, matched_indices)| matched_indices)
        .collect::<Vec<_>>()
}
