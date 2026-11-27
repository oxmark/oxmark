//! Feeds random Markdown-like input to the parser to catch panics.

/// A small xorshift generator, so the test needs no dependency and is
/// reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[rustfmt::skip]
const PIECES: &[&str] = &[
    " ", "  ", "\t", "\n", "\n\n", "\r\n", "\r", "a", "b", "foo", "é", "ẞ", "😀", "\u{a0}", "*", "**", "_",
    "__", "~", "~~", "`", "```", "~~~", "#", "## ", ">", "> ", "-", "- ", "+ ", "1. ", "2) ", "[", "]",
    "![", "(", ")", "<", ">", "</", "<!--", "-->", "<?", "?>", "<![CDATA[", "]]>", "<div>", "<a href='x'>",
    "\\", "&", "&amp;", "&#", "&#x", ";", "\"", "'", ":", "|", "| ", " |", "---", "===", ":-:", "    ",
    "[ ]", "[x]", "www.", "http://", "https://", "@", ".com", "javascript:", "\0", "[a]: /u \"t\"\n",
];

#[test]
fn random_inputs_do_not_panic() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let iterations = if cfg!(debug_assertions) {
        20_000
    } else {
        200_000
    };
    for _ in 0..iterations {
        let len = (rng.next() % 40) as usize;
        let input: String = (0..len)
            .map(|_| PIECES[(rng.next() % PIECES.len() as u64) as usize])
            .collect();
        let result = std::panic::catch_unwind(|| {
            let _ = oxmark::to_html(&input);
            let _ = oxmark::to_html_with(
                &input,
                &oxmark::Options::commonmark(),
                &oxmark::HtmlOptions::trusted(),
            );
        });
        assert!(result.is_ok(), "panic on input {input:?}");
    }
}
