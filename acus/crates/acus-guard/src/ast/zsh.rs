// SPDX-License-Identifier: MPL-2.0
//! Owns the zsh grammar's segmentation. Claude Code runs Bash-tool commands in the user's login
//! shell, so on a Mac a command can use zsh-only syntax (`${(f)x}`, `=(cmd)`, `*(.)`,
//! `repeat 3 { … }`) that the Bash grammar rejects. The grammar is a fork of tree-sitter-bash
//! and builds the same tree for the shared syntax, so the walk in `bash` serves both.

use super::*;

/// The scanner of tree-sitter-zsh 0.63 aborts the whole process (an `assert` in its
/// `deserialize`, not a parse error) once brackets and quotes nest a few hundred levels deep;
/// 250 `echo "$(` do it. No command an agent writes nests this deep, so past this many open
/// brackets the call runs unchanged without the parser seeing it.
const MAX_NESTING: usize = 48;

fn nests_too_deep(src: &str) -> bool {
    let (mut depth, mut deepest) = (0usize, 0usize);
    for b in src.bytes() {
        match b {
            b'(' | b'{' | b'[' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b')' | b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest > MAX_NESTING
}

pub(super) fn parse(src: &str) -> Option<Parsed> {
    if nests_too_deep(src) {
        return None;
    }
    bash::parse(src, &tree_sitter_zsh::LANGUAGE.into())
}
