pub fn draw_bar(done: usize, total: usize, name: &str) {
    const WIDTH: usize = 30;
    let filled = done * WIDTH / total.max(1);

    eprint!(
        "\r{name:<24.24} [{}{}] {done:>3}/{total}",
        "=".repeat(filled),
        " ".repeat(WIDTH - filled),
    );
}
