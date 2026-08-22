//! Syntax highlighting for fenced code blocks.
//!
//! Comrak parses markdown into an AST and calls into a
//! [`SyntaxHighlighterAdapter`] for every fenced code block. We highlight
//! known languages with Syntect (emitting CSS classes, not inline colors, so
//! the client-side theme switcher keeps working) and pass everything else
//! through untouched. In particular `mermaid` blocks must retain their
//! `class="language-mermaid"` marker so the client can transform them into
//! diagrams.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{self, Write};
use std::sync::OnceLock;

use comrak::adapters::SyntaxHighlighterAdapter;
use syntect::html::{css_for_theme_with_class_style, ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Class prefix for every Syntect-generated span, keeping highlight classes
/// namespaced away from the template's own CSS.
const CLASS_STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "syn-" };

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Adapter Comrak invokes for each fenced code block.
pub(crate) struct SyntectAdapter;

impl SyntaxHighlighterAdapter for SyntectAdapter {
    fn write_highlighted(
        &self,
        output: &mut dyn Write,
        lang: Option<&str>,
        code: &str,
    ) -> fmt::Result {
        let lang = lang.unwrap_or("").trim();

        // Mermaid and unknown languages: emit escaped source unchanged. The
        // language class lives on the surrounding <code> tag, so mermaid blocks
        // keep their marker for the client-side transform.
        let syntax = if lang.eq_ignore_ascii_case("mermaid") {
            None
        } else {
            syntax_set().find_syntax_by_token(lang)
        };

        let Some(syntax) = syntax else {
            return write_escaped(output, code);
        };

        let mut generator =
            ClassedHTMLGenerator::new_with_class_style(syntax, syntax_set(), CLASS_STYLE);
        for line in LinesWithEndings::from(code) {
            if generator
                .parse_html_for_line_which_includes_newline(line)
                .is_err()
            {
                // Fall back to plain escaped source on any highlight error.
                return write_escaped(output, code);
            }
        }
        output.write_str(&generator.finalize())
    }

    fn write_pre_tag(
        &self,
        output: &mut dyn Write,
        attributes: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        write_opening_tag(output, "pre", &attributes)
    }

    fn write_code_tag(
        &self,
        output: &mut dyn Write,
        attributes: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        write_opening_tag(output, "code", &attributes)
    }
}

fn write_opening_tag(
    output: &mut dyn Write,
    tag: &str,
    attributes: &HashMap<&'static str, Cow<'_, str>>,
) -> fmt::Result {
    write!(output, "<{tag}")?;
    // Sort keys for deterministic output (tests match on exact attributes).
    let mut keys: Vec<&&str> = attributes.keys().collect();
    keys.sort();
    for key in keys {
        write!(output, " {key}=\"{}\"", attributes[*key])?;
    }
    output.write_char('>')
}

fn write_escaped(output: &mut dyn Write, text: &str) -> fmt::Result {
    for ch in text.chars() {
        match ch {
            '&' => output.write_str("&amp;")?,
            '<' => output.write_str("&lt;")?,
            '>' => output.write_str("&gt;")?,
            '"' => output.write_str("&quot;")?,
            _ => output.write_char(ch)?,
        }
    }
    Ok(())
}

/// CSS for the Syntect highlight classes, scoped per UI theme so the same
/// pre-rendered HTML restyles instantly when the client switches themes.
///
/// Light UI themes use a light Syntect theme; dark ones a dark theme. Only
/// foreground colors are kept — the code block background stays `--code-bg`.
pub(crate) fn highlight_css() -> &'static str {
    static CSS: OnceLock<String> = OnceLock::new();
    CSS.get_or_init(|| {
        let themes = syntect::highlighting::ThemeSet::load_defaults();
        let light = &themes.themes["InspiredGitHub"];
        let dark = &themes.themes["base16-ocean.dark"];

        let light_css = css_for_theme_with_class_style(light, CLASS_STYLE).unwrap_or_default();
        let dark_css = css_for_theme_with_class_style(dark, CLASS_STYLE).unwrap_or_default();

        let mut out = String::new();
        for theme in ["light", "catppuccin-latte"] {
            out.push_str(&scope_css(&light_css, theme));
        }
        for theme in ["dark", "catppuccin-macchiato", "catppuccin-mocha"] {
            out.push_str(&scope_css(&dark_css, theme));
        }
        out
    })
}

/// Prefix every rule in `css` with `html[data-theme="<theme>"]` and drop the
/// base `background-color` so highlighting only sets foreground colors.
fn scope_css(css: &str, theme: &str) -> String {
    let scope = format!("html[data-theme=\"{theme}\"]");
    let css = strip_comments(css);
    let mut out = String::new();

    for rule in css.split('}') {
        let rule = rule.trim();
        if rule.is_empty() {
            continue;
        }
        let Some(brace) = rule.find('{') else {
            continue;
        };
        let selectors = rule[..brace].trim();
        let body = rule[brace + 1..].trim();

        let scoped: Vec<String> = selectors
            .split(',')
            .map(|sel| format!("{scope} {}", sel.trim()))
            .collect();

        // Keep foreground colors, drop backgrounds so --code-bg wins.
        let body: String = body
            .split(';')
            .map(str::trim)
            .filter(|decl| !decl.is_empty() && !decl.starts_with("background"))
            .map(|decl| format!("{decl};"))
            .collect();

        if body.is_empty() {
            continue;
        }

        out.push_str(&scoped.join(", "));
        out.push_str(" { ");
        out.push_str(&body);
        out.push_str(" }\n");
    }
    out
}

fn strip_comments(css: &str) -> String {
    let mut out = String::new();
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the adapter's code-block highlighter and return the inner HTML.
    fn highlight(lang: Option<&str>, code: &str) -> String {
        let mut out = String::new();
        SyntectAdapter
            .write_highlighted(&mut out, lang, code)
            .expect("write_highlighted");
        out
    }

    #[test]
    fn known_language_emits_scope_classes() {
        let html = highlight(Some("rust"), "fn main() {}\n");
        assert!(html.contains("syn-"), "expected highlight spans: {html}");
        assert!(html.contains("main"));
    }

    #[test]
    fn mermaid_passes_through_without_highlighting() {
        let code = "graph TD\n  A-->B\n";
        let html = highlight(Some("mermaid"), code);
        assert!(!html.contains("syn-"), "mermaid must not be highlighted");
        // Content is preserved verbatim (only HTML-escaped) for the client.
        assert!(html.contains("graph TD"));
        assert!(html.contains("A--&gt;B"));
    }

    #[test]
    fn unknown_language_passes_through_escaped() {
        let html = highlight(Some("not-a-language"), "a < b & c\n");
        assert!(!html.contains("syn-"));
        assert_eq!(html, "a &lt; b &amp; c\n");
    }

    #[test]
    fn no_language_passes_through_escaped() {
        let html = highlight(None, "<tag>\n");
        assert!(!html.contains("syn-"));
        assert_eq!(html, "&lt;tag&gt;\n");
    }

    #[test]
    fn code_tag_keeps_language_class() {
        // The language class lives on the <code> tag; this is what lets the
        // client find mermaid blocks after skip-highlighting.
        let mut out = String::new();
        let mut attrs = HashMap::new();
        attrs.insert("class", Cow::Borrowed("language-mermaid"));
        SyntectAdapter
            .write_code_tag(&mut out, attrs)
            .expect("write_code_tag");
        assert_eq!(out, r#"<code class="language-mermaid">"#);
    }

    #[test]
    fn scope_css_prefixes_selector_and_drops_background() {
        let css = ".syn-keyword {\n color: #abc;\n background-color: #000;\n}\n";
        let scoped = scope_css(css, "dark");
        assert!(scoped.contains(r#"html[data-theme="dark"] .syn-keyword"#));
        assert!(scoped.contains("color: #abc;"));
        assert!(!scoped.contains("background"));
    }

    #[test]
    fn highlight_css_covers_every_theme() {
        let css = highlight_css();
        for theme in [
            "light",
            "catppuccin-latte",
            "dark",
            "catppuccin-macchiato",
            "catppuccin-mocha",
        ] {
            assert!(
                css.contains(&format!(r#"html[data-theme="{theme}"]"#)),
                "missing scoped rules for {theme}"
            );
        }
        assert!(
            !css.contains("background"),
            "backgrounds should be stripped"
        );
    }
}
