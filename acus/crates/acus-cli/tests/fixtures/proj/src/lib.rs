pub struct Parser {
    pos: usize,
}

impl Parser {
    pub fn new() -> Self {
        Parser { pos: 0 }
    }

    pub fn parse(&mut self, input: &str) -> usize {
        let needle = input.len();
        self.pos += needle;
        self.pos
    }
}

// needle outside any symbol
