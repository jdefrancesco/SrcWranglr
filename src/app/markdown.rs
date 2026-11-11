//! Markdown (CommonMark, with tables, strikethrough and task lists) read
//! into blocks of styled text for the README window, by pulldown-cmark.
//! A README comes from the audited tree, so nothing in it is followed or
//! loaded: a link keeps its address only to show it, an image becomes its
//! description, and HTML becomes the text inside its tags.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A piece of the README to lay out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A heading, level 1 to 6.
    Heading {
        level: u8,
        runs: Vec<Run>,
    },
    Paragraph(Vec<Run>),
    /// A code block.
    Code(String),
    /// A list item: how deep its list is (0 at the top), its marker ("•",
    /// "3.", "☐", or none for its second paragraph on), and its text.
    Item {
        depth: u8,
        marker: String,
        runs: Vec<Run>,
    },
    /// A paragraph quoted.
    Quote(Vec<Run>),
    /// A horizontal rule.
    Rule,
    /// A table's rows of cells, its header first.
    Table(Vec<Vec<Vec<Run>>>),
}

/// Text in one style.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub style: Style,
    /// The address it links to, if it is a link: shown, never opened.
    pub link: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub is_strong: bool,
    pub is_emphasis: bool,
    pub is_code: bool,
    pub is_struck: bool,
    /// An image's description, in place of the image.
    pub is_image: bool,
}

/// How deep lists are followed; deeper ones are drawn at this depth.
const MAX_DEPTH: u8 = 8;
/// Bounds while reading, so a crafted README can't grow without end
/// before it is cut to what is shown: blocks read, runs and characters
/// in one block (more text joins its last run, or is dropped), bytes of
/// code, HTML or an image's description, and a link's address.
const MAX_BLOCKS_READ: usize = 5_000;
const MAX_BLOCK_RUNS: usize = 500;
const MAX_BLOCK_CHARS: usize = 20_000;
const MAX_TEXT_BYTES: usize = 80_000;
const MAX_LINK_CHARS: usize = 300;

/// `text`, read as Markdown, as blocks.
pub fn blocks(text: &str) -> Vec<Block> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut reader = Reader::default();
    for event in Parser::new_ext(text, options) {
        reader.event(event);
    }
    reader.flush();
    reader.blocks
}

#[derive(Default)]
struct Reader {
    blocks: Vec<Block>,
    /// The text of the block being read.
    runs: Vec<Run>,
    strong: u32,
    emphasis: u32,
    struck: u32,
    links: Vec<String>,
    image: Option<String>,
    heading: Option<u8>,
    code: Option<String>,
    html: Option<String>,
    quotes: u32,
    /// Each list being read: the next item's number, if numbered.
    lists: Vec<Option<u64>>,
    /// The marker of the item being read, until it is shown.
    item: Option<String>,
    /// Inside an HTML tag, across the lines of an HTML block.
    is_in_tag: bool,
    /// The table being read, and its row.
    table: Option<Vec<Vec<Vec<Run>>>>,
    row: Vec<Vec<Run>>,
    /// Characters in the block being read.
    block_chars: usize,
}

impl Reader {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => match self.image.as_mut() {
                Some(description) => description.push_str(&code),
                None => self.push(&code, |s| s.is_code = true),
            },
            Event::Html(html) | Event::InlineHtml(html) => self.html(&html),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Rule => {
                self.flush();
                self.add(Block::Rule);
            }
            Event::TaskListMarker(is_done) => {
                self.item = Some(if is_done { "☑" } else { "☐" }.into());
            }
            Event::FootnoteReference(name) => self.text(&format!("[{name}]")),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush();
                self.heading = Some(heading_level(level));
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.show_marker();
                self.code = Some(String::new());
            }
            Tag::HtmlBlock => {
                self.flush();
                self.show_marker();
                self.html = Some(String::new());
                self.is_in_tag = false;
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.show_marker();
                self.quotes += 1;
            }
            Tag::List(start) => {
                // An item's text before its own list comes out first.
                self.flush();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}.");
                        *number += 1;
                        marker
                    }
                    _ => "•".into(),
                };
                self.item = Some(marker);
            }
            Tag::Table(_) => {
                self.flush();
                self.show_marker();
                self.table = Some(Vec::new());
            }
            Tag::TableHead | Tag::TableRow => self.row.clear(),
            Tag::TableCell => self.runs.clear(),
            Tag::Strong => self.strong += 1,
            Tag::Emphasis => self.emphasis += 1,
            Tag::Strikethrough => self.struck += 1,
            Tag::Link { dest_url, .. } => {
                let address: String = dest_url.chars().take(MAX_LINK_CHARS).collect();
                self.links.push(address);
            }
            Tag::Image { .. } => self.image = Some(String::new()),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(1);
                let runs = std::mem::take(&mut self.runs);
                self.add(Block::Heading { level, runs });
            }
            TagEnd::CodeBlock => {
                let code = self.code.take().unwrap_or_default();
                self.add(Block::Code(code.trim_end_matches('\n').to_string()));
            }
            TagEnd::HtmlBlock => {
                let text = self.html.take().unwrap_or_default();
                let text = text.trim();
                if !text.is_empty() {
                    let runs = vec![plain(text)];
                    self.add(Block::Paragraph(runs));
                }
            }
            TagEnd::Paragraph => {
                self.flush();
                self.is_in_tag = false;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quotes = self.quotes.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush();
                // An item with no text still shows its marker.
                self.show_marker();
                self.item = None;
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.runs);
                self.row.push(cell);
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                let row = std::mem::take(&mut self.row);
                if let Some(table) = self.table.as_mut() {
                    table.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.add(Block::Table(table));
                }
            }
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Emphasis => self.emphasis = self.emphasis.saturating_sub(1),
            TagEnd::Strikethrough => self.struck = self.struck.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => {
                let description = self.image.take().unwrap_or_default();
                let description = description.trim();
                let text = match description.is_empty() {
                    true => "[image]".to_string(),
                    false => format!("[image: {description}]"),
                };
                self.push(&text, |s| s.is_image = true);
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = self.code.as_mut() {
            push_bounded(code, text);
        } else if let Some(description) = self.image.as_mut() {
            push_bounded(description, text);
        } else {
            self.push(text, |_| {});
        }
    }

    /// Adds `block`, unless enough have been read.
    fn add(&mut self, block: Block) {
        self.block_chars = 0;
        if self.blocks.len() < MAX_BLOCKS_READ {
            self.blocks.push(block);
        }
    }

    /// The item's marker on a line of its own, if its first part isn't
    /// text (a code block, a table) or it has none.
    fn show_marker(&mut self) {
        if self.lists.is_empty() || !self.runs.is_empty() || self.item.is_none() {
            return;
        }
        self.runs.push(Run::default());
        self.flush();
    }

    /// HTML in the README: the text between its tags.
    fn html(&mut self, html: &str) {
        let text = without_tags(html, &mut self.is_in_tag);
        if let Some(block) = self.html.as_mut() {
            push_bounded(block, &text);
        } else if !text.trim().is_empty() {
            self.push(&text, |_| {});
        }
    }

    /// Adds `text` in the current style, changed by `restyle`.
    fn push(&mut self, text: &str, restyle: impl FnOnce(&mut Style)) {
        let room = MAX_BLOCK_CHARS.saturating_sub(self.block_chars);
        if text.is_empty() || room == 0 || self.blocks.len() >= MAX_BLOCKS_READ {
            return;
        }
        let text: String = text.chars().take(room).collect();
        self.block_chars += text.chars().count();
        let text = text.as_str();
        let mut style = Style {
            is_strong: self.strong > 0,
            is_emphasis: self.emphasis > 0,
            is_struck: self.struck > 0,
            ..Style::default()
        };
        restyle(&mut style);
        let link = self.links.last().cloned();
        let is_full = self.runs.len() >= MAX_BLOCK_RUNS;
        match self.runs.last_mut() {
            // Past the runs a block may have, the rest is its last run's.
            Some(last) if is_full || (last.style == style && last.link == link) => {
                last.text.push_str(text);
            }
            _ => self.runs.push(Run {
                text: text.to_string(),
                style,
                link,
            }),
        }
    }

    /// Puts the text read so far in a block of its own: a list item's,
    /// a quote's, a table cell's (kept for its row), or a paragraph.
    fn flush(&mut self) {
        if self.table.is_some() || self.heading.is_some() || self.runs.is_empty() {
            return;
        }
        let runs = std::mem::take(&mut self.runs);
        let block = if !self.lists.is_empty() {
            let depth = u8::try_from(self.lists.len() - 1).unwrap_or(MAX_DEPTH);
            Block::Item {
                depth: depth.min(MAX_DEPTH),
                // Its first paragraph has the marker; the rest line up.
                marker: self.item.take().unwrap_or_default(),
                runs,
            }
        } else if self.quotes > 0 {
            Block::Quote(runs)
        } else {
            Block::Paragraph(runs)
        };
        self.add(block);
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn plain(text: &str) -> Run {
    Run {
        text: text.to_string(),
        ..Run::default()
    }
}

/// `text` added to `to`, until `to` holds `MAX_TEXT_BYTES`.
fn push_bounded(to: &mut String, text: &str) {
    let room = MAX_TEXT_BYTES.saturating_sub(to.len());
    let mut end = room.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    to.push_str(&text[..end]);
}

/// `html` without its tags (and comments), carrying `is_in_tag` over from
/// the HTML before it (a tag may go on over lines).
fn without_tags(html: &str, is_in_tag: &mut bool) -> String {
    let mut out = String::with_capacity(html.len());
    for c in html.chars() {
        match c {
            '<' => *is_in_tag = true,
            '>' if *is_in_tag => *is_in_tag = false,
            c if !*is_in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
