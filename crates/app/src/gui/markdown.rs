//! Markdown, parsed into blocks and rendered for reading.
//!
//! # Why this exists rather than a crate
//!
//! What is displayed here is a file out of a repository that may not be the
//! user's — a README from a dependency, a LICENSE from a clone, a document
//! pulled back out of a backup. The safe set of things such a file may do is
//! very small, and it is easier to be sure of that by writing the small thing
//! than by disabling most of a general Markdown implementation.
//!
//! So: no HTML, no images, no scripts, no `javascript:` links, and nothing
//! fetched. Links render with their destination visible and open only when the
//! reader clicks them.
//!
//! # What it covers
//!
//! Headings, paragraphs, bullet and numbered lists with nesting, fenced and
//! indented code, block quotes, horizontal rules, pipe tables, and the inline
//! run of bold, italic, inline code and links. That is what READMEs, licences
//! and changelogs are made of. Anything it does not recognise is shown as the
//! text it is, which is the failure mode a reader can work with.

use egui::Ui;

use super::theme::{self, space, Type};
use super::widgets;

/// A run of inline text, with whatever formatting applies to it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// Where a link goes. Shown to the reader; opened only on a click.
    pub link: Option<String>,
}

/// One block of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, spans: Vec<Span> },
    Paragraph(Vec<Span>),
    /// `indent` is the nesting depth, `marker` the bullet or number shown.
    ListItem { indent: usize, marker: String, spans: Vec<Span> },
    Code { language: Option<String>, lines: Vec<String> },
    Quote(Vec<Span>),
    Rule,
    Table { headers: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
}

/// Parse a document into blocks.
pub fn parse(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut i = 0;

    // A paragraph accumulates until something ends it, so that a sentence
    // wrapped over three source lines renders as one flowing paragraph rather
    // than three stubs.
    macro_rules! flush {
        () => {
            if !paragraph.is_empty() {
                blocks.push(Block::Paragraph(parse_spans(&paragraph.join(" "))));
                paragraph.clear();
            }
        };
    }

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // Fenced code. The fence's own line carries the language.
        if let Some(rest) = trimmed.strip_prefix("```").or_else(|| trimmed.strip_prefix("~~~")) {
            flush!();
            let language = rest.split_whitespace().next().map(str::to_string);
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() {
                let candidate = lines[i].trim_start();
                if candidate.starts_with("```") || candidate.starts_with("~~~") {
                    break;
                }
                body.push(lines[i].to_string());
                i += 1;
            }
            blocks.push(Block::Code { language: language.filter(|l| !l.is_empty()), lines: body });
            i += 1;
            continue;
        }

        if trimmed.is_empty() {
            flush!();
            i += 1;
            continue;
        }

        // A rule: three or more of one of `-`, `*`, `_` and nothing else.
        if trimmed.len() >= 3
            && (trimmed.chars().all(|c| c == '-')
                || trimmed.chars().all(|c| c == '*')
                || trimmed.chars().all(|c| c == '_'))
        {
            flush!();
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }

        // A heading.
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed.chars().nth(hashes) == Some(' ') {
            flush!();
            blocks.push(Block::Heading {
                level: hashes as u8,
                spans: parse_spans(trimmed[hashes + 1..].trim()),
            });
            i += 1;
            continue;
        }

        // A `Setext` heading: text with `===` or `---` under it.
        if i + 1 < lines.len() && !trimmed.is_empty() {
            let under = lines[i + 1].trim();
            if under.len() >= 3 && (under.chars().all(|c| c == '=') || under.chars().all(|c| c == '-'))
            {
                flush!();
                let level = if under.starts_with('=') { 1 } else { 2 };
                blocks.push(Block::Heading { level, spans: parse_spans(trimmed) });
                i += 2;
                continue;
            }
        }

        // A pipe table: a header row, a separator of dashes, then rows.
        if trimmed.contains('|') && i + 1 < lines.len() && is_table_separator(lines[i + 1]) {
            flush!();
            let headers = table_cells(trimmed);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(table_cells(lines[i]));
                i += 1;
            }
            blocks.push(Block::Table { headers, rows });
            continue;
        }

        // A block quote.
        if let Some(rest) = trimmed.strip_prefix('>') {
            flush!();
            blocks.push(Block::Quote(parse_spans(rest.trim())));
            i += 1;
            continue;
        }

        // A list item, bulleted or numbered.
        let indent = (line.len() - trimmed.len()) / 2;
        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            flush!();
            blocks.push(Block::ListItem {
                indent,
                marker: "•".to_string(),
                spans: parse_spans(item),
            });
            i += 1;
            continue;
        }
        if let Some((number, rest)) = numbered_item(trimmed) {
            flush!();
            blocks.push(Block::ListItem {
                indent,
                marker: format!("{number}."),
                spans: parse_spans(rest),
            });
            i += 1;
            continue;
        }

        paragraph.push(trimmed.to_string());
        i += 1;
    }
    flush!();
    blocks
}

/// `1. text` or `1) text`.
fn numbered_item(line: &str) -> Option<(u32, &str)> {
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &line[digits.len()..];
    let rest = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?;
    Some((digits.parse().ok()?, rest))
}

/// `|---|:--:|` and friends.
fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('-')
        && trimmed.contains('|')
        && trimmed.chars().all(|c| matches!(c, '-' | '|' | ':' | ' '))
}

fn table_cells(line: &str) -> Vec<Vec<Span>> {
    line.trim().trim_matches('|').split('|').map(|cell| parse_spans(cell.trim())).collect()
}

/// Parse the inline run: bold, italic, inline code, and links.
///
/// A single pass with a small state machine rather than a regex, because the
/// markers nest and overlap in ways a regex reads badly — and because getting
/// an unclosed marker wrong should leave the characters on screen, not eat the
/// rest of the paragraph.
///
/// Three rules do most of the work, and each of them is a bug that showed up
/// in a real document:
///
/// * **A marker only opens if something closes it.** Otherwise a lone asterisk
///   in `2 * 3` starts an italic run that swallows everything after it.
/// * **An opener is followed by a non-space and a closer preceded by one.**
///   This is CommonMark's flanking rule, cut down. It is what stops `2 * 3`
///   and `a stray ** here` from being markup at all.
/// * **A closer needs no lookahead.** Asking whether *another* marker follows
///   made the second `**` of `**bold**` fail to close, so the run stayed open
///   and one asterisk leaked into the text.
///
/// Underscores additionally never open or close inside a word, so
/// `some_variable_name` is a name rather than an italic run.
pub fn parse_spans(source: &str) -> Vec<Span> {
    let chars: Vec<char> = source.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut current = String::new();
    let mut bold = false;
    let mut italic = false;
    let mut i = 0;

    macro_rules! push {
        () => {
            if !current.is_empty() {
                spans.push(Span {
                    text: std::mem::take(&mut current),
                    bold,
                    italic,
                    code: false,
                    link: None,
                });
            }
        };
    }

    while i < chars.len() {
        // Inline code wins over everything: markers inside backticks are
        // literal, which is the whole point of writing `**` in a README about
        // Markdown.
        if chars[i] == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|c| *c == '`') {
                push!();
                spans.push(Span {
                    text: chars[i + 1..i + 1 + end].iter().collect(),
                    code: true,
                    ..Default::default()
                });
                i += end + 2;
                continue;
            }
        }

        if chars[i] == '[' {
            if let Some((text, url, next)) = link_at(&chars, i) {
                push!();
                // Only somewhere a reader can safely be sent. A `javascript:`
                // or `data:` URL in a README from a repository that is not the
                // user's is not a link, it is an attempt — and the words are
                // still shown, so nothing is silently swallowed.
                let safe = url.starts_with("http://")
                    || url.starts_with("https://")
                    || url.starts_with("mailto:");
                spans.push(Span {
                    text,
                    bold,
                    italic,
                    code: false,
                    link: safe.then_some(url),
                });
                i = next;
                continue;
            }
        }

        let c = chars[i];
        if c == '*' || c == '_' {
            let double = chars.get(i + 1) == Some(&c);
            let run = if double { 2 } else { 1 };
            let after = chars.get(i + run).copied();
            let before = i.checked_sub(1).and_then(|p| chars.get(p)).copied();
            let active = if double { bold } else { italic };

            // An underscore inside a word is part of the word.
            let intraword = c == '_'
                && (before.is_some_and(|p| p.is_alphanumeric())
                    || after.is_some_and(|n| n.is_alphanumeric() && active));

            let closes = active && before.is_some_and(|p| !p.is_whitespace()) && !intraword;
            let opens = !active
                && after.is_some_and(|n| !n.is_whitespace())
                && !intraword
                && has_closer(&chars, i + run, c, run);

            if closes || opens {
                push!();
                if double {
                    bold = !bold;
                } else {
                    italic = !italic;
                }
                i += run;
                continue;
            }
        }

        current.push(c);
        i += 1;
    }
    push!();
    spans
}

/// Is there a marker later that could close this one?
///
/// Only asked when *opening*. A closer that is preceded by a space is not a
/// closer, so this looks for one that is not.
fn has_closer(chars: &[char], from: usize, marker: char, run: usize) -> bool {
    let mut i = from;
    while i + run <= chars.len() {
        let matches = chars[i] == marker && (run == 1 || chars.get(i + 1) == Some(&marker));
        let preceded_by_text =
            i.checked_sub(1).and_then(|p| chars.get(p)).is_some_and(|p| !p.is_whitespace());
        if matches && preceded_by_text {
            return true;
        }
        i += 1;
    }
    false
}

/// `[text](url)`, with the parentheses inside the URL counted.
///
/// Stopping at the first `)` split `[click](javascript:alert(1))` in the
/// middle, leaving a stray bracket in the text — and a URL truncated to
/// something that no longer looked like the scheme it was.
fn link_at(chars: &[char], open: usize) -> Option<(String, String, usize)> {
    let close = chars.iter().skip(open).position(|c| *c == ']')? + open;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let mut depth = 1usize;
    let mut end = close + 2;
    while end < chars.len() {
        match chars[end] {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        end += 1;
    }
    if depth != 0 {
        return None;
    }
    let text: String = chars[open + 1..close].iter().collect();
    let url: String = chars[close + 2..end].iter().collect();
    Some((text, url.trim().to_string(), end + 1))
}

/// Render a parsed document. Returns a URL if the reader clicked a link.
///
/// Nothing is opened here: a click is reported and the caller decides, so the
/// one place that can send a reader to a web address is a place that knows
/// where the document came from.
pub fn render(ui: &mut Ui, blocks: &[Block], width: f32) -> Option<String> {
    let t = theme::tokens(ui.ctx());
    let mut clicked = None;

    for block in blocks {
        match block {
            Block::Heading { level, spans } => {
                ui.add_space(if *level <= 2 { space::L } else { space::M });
                let ty = match level {
                    1 => Type::H1,
                    2 => Type::H2,
                    _ => Type::H3,
                };
                if let Some(url) = inline(ui, spans, ty, t.text_primary, width) {
                    clicked = Some(url);
                }
                // A rule under the top two levels, which is how they read in
                // every rendered README the user has ever seen.
                if *level <= 2 {
                    ui.add_space(space::XS);
                    widgets::divider(ui);
                }
                ui.add_space(space::XS);
            }
            Block::Paragraph(spans) => {
                if let Some(url) = inline(ui, spans, Type::Body, t.text_secondary, width) {
                    clicked = Some(url);
                }
                ui.add_space(space::M);
            }
            Block::ListItem { indent, marker, spans } => {
                ui.horizontal_top(|ui| {
                    ui.add_space(space::M + *indent as f32 * space::XL);
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(20.0, 18.0),
                        egui::Layout::right_to_left(egui::Align::Min),
                        |ui| {
                            widgets::text(ui, marker, Type::Small, t.text_muted);
                        },
                    );
                    ui.add_space(space::S);
                    let room = width - space::M - *indent as f32 * space::XL - 32.0;
                    if let Some(url) =
                        inline(ui, spans, Type::Body, t.text_secondary, room.max(80.0))
                    {
                        clicked = Some(url);
                    }
                });
                ui.add_space(space::XS);
            }
            Block::Code { language, lines } => {
                let body = lines.join("\n");
                egui::Frame::new()
                    .fill(t.bg_code)
                    .stroke(egui::Stroke::new(1.0_f32, t.border_subtle))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(width - 24.0);
                        if let Some(language) = language {
                            widgets::text(ui, language, Type::MonoSmall, t.text_muted);
                            ui.add_space(space::XS);
                        }
                        // Horizontal scroll rather than wrapping: a wrapped
                        // command line is a command line you cannot copy.
                        egui::ScrollArea::horizontal().id_salt(&body).show(ui, |ui| {
                            for line in lines {
                                widgets::text(ui, line, Type::Mono, t.text_primary);
                            }
                        });
                    });
                ui.add_space(space::M);
            }
            Block::Quote(spans) => {
                ui.horizontal_top(|ui| {
                    ui.add_space(space::M);
                    widgets::vertical_rule(ui, 18.0);
                    ui.add_space(space::M);
                    if let Some(url) =
                        inline(ui, spans, Type::Body, t.text_muted, (width - 48.0).max(80.0))
                    {
                        clicked = Some(url);
                    }
                });
                ui.add_space(space::M);
            }
            Block::Rule => {
                ui.add_space(space::M);
                widgets::divider(ui);
                ui.add_space(space::M);
            }
            Block::Table { headers, rows } => {
                let columns = headers.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
                if columns == 0 {
                    continue;
                }
                let cell = ((width - 24.0) / columns as f32).max(60.0);
                widgets::table_frame(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for header in headers {
                            ui.allocate_ui_with_layout(
                                egui::Vec2::new(cell, 20.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    inline(ui, header, Type::BodyStrong, t.text_primary, cell - 8.0);
                                },
                            );
                        }
                    });
                    widgets::divider(ui);
                    for row in rows {
                        ui.horizontal_top(|ui| {
                            for value in row {
                                ui.allocate_ui_with_layout(
                                    egui::Vec2::new(cell, 20.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        if let Some(url) = inline(
                                            ui,
                                            value,
                                            Type::Small,
                                            t.text_secondary,
                                            cell - 8.0,
                                        ) {
                                            clicked = Some(url);
                                        }
                                    },
                                );
                            }
                        });
                    }
                });
                ui.add_space(space::M);
            }
        }
    }
    clicked
}

/// Lay out one run of spans, wrapping, with the formatting each carries.
///
/// A word at a time inside a wrapping row. egui can lay out a mixed-format
/// paragraph in one go as a `LayoutJob`, but a job is a single widget and a
/// link inside it cannot be clicked on its own — and being able to see and
/// follow a link is most of what makes a README readable.
fn inline(
    ui: &mut Ui,
    spans: &[Span],
    ty: Type,
    colour: egui::Color32,
    width: f32,
) -> Option<String> {
    let t = theme::tokens(ui.ctx());
    let mut clicked = None;
    ui.allocate_ui_with_layout(
        egui::Vec2::new(width.max(60.0), 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.spacing_mut().item_spacing.y = 2.0;
                for span in spans {
                    for word in span.text.split_inclusive(' ') {
                        if word.is_empty() {
                            continue;
                        }
                        let mut rich = egui::RichText::new(word).font(ty.font());
                        if span.bold {
                            rich = rich.strong();
                        }
                        if span.italic {
                            rich = rich.italics();
                        }
                        if span.code {
                            rich = rich
                                .font(Type::Mono.font())
                                .background_color(t.bg_code)
                                .color(t.text_primary);
                        } else if span.link.is_some() {
                            rich = rich.color(t.accent).underline();
                        } else {
                            rich = rich.color(colour);
                        }

                        match &span.link {
                            Some(url) => {
                                // The destination is on hover, so following a
                                // link out of somebody else's README is a
                                // decision made with the address in view.
                                let label =
                                    egui::Label::new(rich).sense(egui::Sense::click());
                                if ui.add(label).on_hover_text(url).clicked() {
                                    clicked = Some(url.clone());
                                }
                            }
                            None => {
                                ui.label(rich);
                            }
                        }
                    }
                }
            });
        },
    );
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// The shapes a README is made of, in one document.
    #[test]
    fn a_readme_parses_into_the_blocks_it_is_made_of() {
        let source = "\
# superbackup

Backups for machines **full of code**.

## Install

1. Download the release
2. Run it

- A bullet
  - A nested bullet

> Not a warning, a note.

```bash
cargo build --release
```

---

| Platform | State |
|---|---|
| Windows | done |
| Linux | soon |
";
        let blocks = parse(source);

        assert!(matches!(&blocks[0], Block::Heading { level: 1, .. }));
        let Block::Paragraph(spans) = &blocks[1] else { panic!("{:?}", blocks[1]) };
        assert_eq!(text_of(spans), "Backups for machines full of code.");
        assert!(spans.iter().any(|s| s.bold && s.text.contains("full")), "{spans:?}");

        assert!(matches!(&blocks[2], Block::Heading { level: 2, .. }));

        // Numbered items keep their numbers; bullets get a bullet.
        let numbered: Vec<&Block> = blocks
            .iter()
            .filter(|b| matches!(b, Block::ListItem { marker, .. } if marker.ends_with('.')))
            .collect();
        assert_eq!(numbered.len(), 2);
        let Block::ListItem { marker, .. } = numbered[1] else { panic!() };
        assert_eq!(marker, "2.");

        // Nesting comes from the indent.
        let nested = blocks
            .iter()
            .find_map(|b| match b {
                Block::ListItem { indent, spans, .. } if *indent > 0 => Some((indent, spans)),
                _ => None,
            })
            .expect("a nested bullet");
        assert_eq!(*nested.0, 1);
        assert_eq!(text_of(nested.1), "A nested bullet");

        assert!(blocks.iter().any(|b| matches!(b, Block::Quote(_))));
        assert!(blocks.iter().any(|b| matches!(b, Block::Rule)));

        let code = blocks
            .iter()
            .find_map(|b| match b {
                Block::Code { language, lines } => Some((language, lines)),
                _ => None,
            })
            .expect("a code block");
        assert_eq!(code.0.as_deref(), Some("bash"));
        assert_eq!(code.1, &vec!["cargo build --release".to_string()]);

        let table = blocks
            .iter()
            .find_map(|b| match b {
                Block::Table { headers, rows } => Some((headers, rows)),
                _ => None,
            })
            .expect("a table");
        assert_eq!(table.0.len(), 2);
        assert_eq!(table.1.len(), 2);
        assert_eq!(text_of(&table.1[1][0]), "Linux");
    }

    /// A sentence wrapped over several source lines is one paragraph, not
    /// three stubs — which is how nearly every README is actually written.
    #[test]
    fn wrapped_source_lines_become_one_paragraph() {
        let blocks = parse("One line\nand its continuation.\n\nA second paragraph.");
        assert_eq!(blocks.len(), 2);
        let Block::Paragraph(spans) = &blocks[0] else { panic!() };
        assert_eq!(text_of(spans), "One line and its continuation.");
    }

    /// Markers that never close are characters, not markup. Without this a
    /// lone asterisk in "2 * 3" opens an italic run that eats the paragraph.
    #[test]
    fn an_unclosed_marker_stays_a_character() {
        let spans = parse_spans("2 * 3 = 6 and a stray ** here");
        assert_eq!(text_of(&spans), "2 * 3 = 6 and a stray ** here");
        assert!(spans.iter().all(|s| !s.italic && !s.bold), "{spans:?}");
    }

    /// Backticks win: a `**` inside inline code is text, which is the whole
    /// reason people write it that way in a README about Markdown.
    #[test]
    fn markers_inside_inline_code_are_literal() {
        let spans = parse_spans("write `**bold**` for bold");
        let code = spans.iter().find(|s| s.code).expect("a code span");
        assert_eq!(code.text, "**bold**");
        assert!(!code.bold);
    }

    /// A link is text plus a destination — and only to somewhere a reader can
    /// safely be sent. A `javascript:` URL in a README from a repository that
    /// is not the user's is not a link, it is an attempt.
    #[test]
    fn only_addresses_a_reader_can_safely_follow_become_links() {
        let ok = parse_spans("see [the docs](https://example.com/docs) for more");
        let link = ok.iter().find(|s| s.link.is_some()).expect("a link");
        assert_eq!(link.text, "the docs");
        assert_eq!(link.link.as_deref(), Some("https://example.com/docs"));

        for hostile in [
            "[click](javascript:alert(1))",
            "[click](data:text/html,<script>)",
            "[click](file:///etc/passwd)",
            "[click](vbscript:x)",
        ] {
            let spans = parse_spans(hostile);
            assert!(
                spans.iter().all(|s| s.link.is_none()),
                "{hostile} must not become a link: {spans:?}"
            );
            // The words survive, so the reader still sees what was written.
            assert_eq!(text_of(&spans), "click");
        }

        // A relative link inside a repository goes nowhere we can resolve, so
        // it is text rather than a broken destination.
        let relative = parse_spans("[the guide](./GUIDE.md)");
        assert!(relative.iter().all(|s| s.link.is_none()));
    }

    /// Nothing here renders HTML, so a document containing it shows the
    /// source rather than acting on it.
    #[test]
    fn html_is_shown_rather_than_interpreted() {
        let blocks = parse("<script>alert(1)</script>\n\n<b>not bold</b>");
        let Block::Paragraph(spans) = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(text_of(spans), "<script>alert(1)</script>");
        let Block::Paragraph(spans) = &blocks[1] else { panic!("{blocks:?}") };
        assert_eq!(text_of(spans), "<b>not bold</b>");
    }

    /// An unterminated fence is the rest of the file, not a panic and not a
    /// silently dropped tail.
    #[test]
    fn an_unterminated_code_fence_takes_the_rest_of_the_document() {
        let blocks = parse("# Title\n\n```\nline one\nline two\n");
        let Block::Code { lines, .. } = &blocks[1] else { panic!("{blocks:?}") };
        assert_eq!(lines, &vec!["line one".to_string(), "line two".to_string()]);
    }

    #[test]
    fn an_empty_document_is_empty_rather_than_a_panic() {
        assert!(parse("").is_empty());
        assert!(parse("\n\n\n").is_empty());
        assert!(parse_spans("").is_empty());
    }
}
