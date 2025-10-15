fn main() {
    // Embed HTML templates at compile-time so runtime rendering stays self-contained.
    minijinja_embed::embed_templates!("templates", &[".html"]);
}
