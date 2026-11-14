// SPDX-License-Identifier: MPL-2.0
//! The `--head` budget: prints lines until the cap, then counts the rest for the footer.

/// `--head N` budget: prints lines until N are out, then counts the rest for the footer.
pub struct Budget {
    head: Option<usize>,
    printed: usize,
    dropped: usize,
}

impl Budget {
    pub fn new(head: Option<usize>) -> Self {
        Budget {
            head,
            printed: 0,
            dropped: 0,
        }
    }

    /// Prints `text` line by line to stdout while under the head; counts the rest.
    pub fn emit(&mut self, text: &str) {
        for line in text.strip_suffix('\n').unwrap_or(text).split('\n') {
            if self.exhausted() {
                self.dropped += 1;
            } else {
                outln!("{line}");
                self.printed += 1;
            }
        }
    }

    pub fn exhausted(&self) -> bool {
        self.head.is_some_and(|n| self.printed >= n)
    }

    /// Names the rerun when lines were dropped. It is printed after the output, outside the head
    /// count.
    pub fn footer(&self) -> Option<String> {
        let n = self.head?;
        (self.dropped > 0).then(|| {
            format!(
                "… output cut at --head {n}; {} more lines (rerun with --head {})",
                self.dropped,
                n + self.dropped
            )
        })
    }
}
