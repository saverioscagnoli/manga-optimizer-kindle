use std::fmt::Write;

pub fn draw_bar(done: usize, total: usize, name: &str) {
    const WIDTH: usize = 30;
    let filled = done * WIDTH / total.max(1);

    eprint!(
        "\r{name:<24.24} [{}{}] {done:>3}/{total}",
        "=".repeat(filled),
        " ".repeat(WIDTH - filled),
    );
}

pub fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r#"\\"#),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            c if c <= '\u{1f}' => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
