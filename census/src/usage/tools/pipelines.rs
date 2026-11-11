// SPDX-License-Identifier: MPL-2.0
//! Splits a shell command into pipelines and stages, ignoring quoted text and heredoc bodies.

/// Splits a command into pipelines of stages at `&&`, `||`, `;`, newlines and `|`,
/// ignoring quoted text and heredoc bodies. The full text is returned for python code checks.
pub(super) fn pipelines(cmd: &str) -> Vec<Vec<Vec<String>>> {
    let mut out = vec![vec![]];
    let mut words: Vec<String> = vec![];
    let (mut word, mut quote, mut has_word) = (String::new(), None::<char>, false);
    let mut heredoc: Option<String> = None;
    let cs: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    macro_rules! end_word {
        () => {{
            if has_word {
                words.push(std::mem::take(&mut word));
                has_word = false;
            }
        }};
    }
    macro_rules! end_stage {
        () => {{
            end_word!();
            if !words.is_empty()
                && let Some(pipeline) = out.last_mut()
            {
                pipeline.push(std::mem::take(&mut words));
            }
        }};
    }
    macro_rules! end_pipeline {
        () => {{
            end_stage!();
            if out.last().is_some_and(|stages| !stages.is_empty()) {
                out.push(vec![]);
            }
        }};
    }
    while i < cs.len() {
        let c = cs[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' && i + 1 < cs.len() {
                i += 1;
                word.push(cs[i]);
            } else {
                word.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                has_word = true;
            }
            '\\' if i + 1 < cs.len() => {
                i += 1;
                word.push(cs[i]);
                has_word = true;
            }
            '<' if cs.get(i + 1) == Some(&'<') && cs.get(i + 2) != Some(&'<') => {
                let (tag, next) = heredoc_tag(&cs, i);
                heredoc = Some(tag);
                i = next;
                continue;
            }
            '\n' => {
                end_pipeline!();
                if let Some(tag) = heredoc.take() {
                    i = skip_heredoc_body(&cs, i, &tag);
                }
            }
            ';' => end_pipeline!(),
            '&' if cs.get(i + 1) == Some(&'&') => {
                end_pipeline!();
                i += 1;
            }
            '|' if cs.get(i + 1) == Some(&'|') => {
                end_pipeline!();
                i += 1;
            }
            '|' => end_stage!(),
            c if c.is_whitespace() => end_word!(),
            c => {
                word.push(c);
                has_word = true;
            }
        }
        i += 1;
    }
    if has_word {
        words.push(word);
    }
    if !words.is_empty()
        && let Some(last) = out.last_mut()
    {
        last.push(words);
    }
    out.retain(|p| !p.is_empty());
    out
}

/// The delimiter of the heredoc that opens at `i`, and the index just past the opener.
fn heredoc_tag(cs: &[char], i: usize) -> (String, usize) {
    let mut j = i + 2;
    while cs
        .get(j)
        .is_some_and(|c| matches!(c, '-' | ' ' | '\'' | '"'))
    {
        j += 1;
    }
    let tag: String = cs[j..]
        .iter()
        .take_while(|c| c.is_alphanumeric() || **c == '_')
        .collect();
    let mut next = j + tag.chars().count();
    if matches!(cs.get(next), Some('\'' | '"')) {
        next += 1;
    }
    (tag, next)
}

/// The index where the heredoc body after line `i` ends: its line that is just `tag`, or the
/// end.
fn skip_heredoc_body(cs: &[char], i: usize, tag: &str) -> usize {
    let mut j = i + 1;
    while j <= cs.len() {
        let e = cs[j..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(cs.len(), |p| j + p);
        if cs[j..e].iter().collect::<String>().trim() == tag || e >= cs.len() {
            return e;
        }
        j = e + 1;
    }
    i
}
