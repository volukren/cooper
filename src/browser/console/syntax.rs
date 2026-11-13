use std::ops::Range;

use rune::{Color, Span};

use crate::{
    connection::DbKind,
    highlight::{Highlighter, Token},
    theme::Theme,
};

const TOKENS: [Token; 8] = [
    Token::Plain,
    Token::Keyword,
    Token::Type,
    Token::Function,
    Token::String,
    Token::Number,
    Token::Comment,
    Token::Identifier,
];

fn slot(token: Token) -> usize {
    TOKENS.iter().position(|t| *t == token).unwrap_or(0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    pub color: Option<Color>,
    pub bold: bool,
    pub italic: bool,
}

impl Style {
    fn span(self, range: Range<usize>) -> Span {
        let mut span = Span::new(range.start, range.end);
        if let Some(color) = self.color {
            span = span.color(color);
        }
        if self.bold {
            span = span.bold();
        }
        if self.italic {
            span = span.italic();
        }
        span
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Palette {
    styles: [Option<Style>; TOKENS.len()],
}

impl Palette {
    pub fn new(t: &Theme) -> Self {
        Self {
            styles: TOKENS.map(|token| token_style(token, t)),
        }
    }

    pub fn style(&self, token: Token) -> Option<Style> {
        self.styles[slot(token)]
    }
}

pub fn token_style(token: Token, t: &Theme) -> Option<Style> {
    t.token_color(token).map(|color| Style {
        color: Some(color),
        bold: token == Token::Keyword,
        italic: token == Token::Comment,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEdit {
    pub start_row: usize,
    pub old_end_row: usize,
    pub new_end_row: usize,
}

pub fn line_edit(old: &str, new: &str) -> Option<LineEdit> {
    if old == new {
        return None;
    }
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let room = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(room)
        .take_while(|(x, y)| x == y)
        .count();
    let rows = |bytes: &[u8]| bytes.iter().filter(|&&c| c == b'\n').count();
    Some(LineEdit {
        start_row: rows(&b[..prefix]),
        old_end_row: rows(&a[..a.len() - suffix]),
        new_end_row: rows(&b[..b.len() - suffix]),
    })
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}

fn line_at<'a>(text: &'a str, starts: &[usize], row: usize) -> &'a str {
    let start = starts[row];
    let end = starts.get(row + 1).map_or(text.len(), |next| next - 1);
    &text[start..end]
}

#[derive(Debug, Clone)]
struct Line {
    start: Highlighter,
    tokens: Vec<(usize, Token)>,
}

#[derive(Debug)]
pub struct SqlLines {
    kind: DbKind,
    lines: Vec<Line>,
    text: String,
    starts: Vec<usize>,
}

impl SqlLines {
    pub fn new(kind: DbKind) -> Self {
        Self {
            kind,
            lines: Vec::new(),
            text: String::new(),
            starts: vec![0],
        }
    }

    pub fn sync(&mut self, kind: DbKind, text: &str) -> usize {
        if kind != self.kind {
            *self = Self::new(kind);
        }
        if !self.lines.is_empty() && self.text == text {
            return 0;
        }
        let edit = line_edit(&self.text, text);
        self.update(edit.as_ref(), text)
    }

    pub fn update(&mut self, edit: Option<&LineEdit>, text: &str) -> usize {
        let starts = line_starts(text);
        let lexed = edit
            .filter(|_| !self.lines.is_empty())
            .and_then(|edit| self.relex(edit, text, &starts));
        let lexed = lexed.unwrap_or_else(|| self.relex_all(text, &starts));
        self.text = text.to_string();
        self.starts = starts;
        lexed
    }

    fn relex(&mut self, edit: &LineEdit, text: &str, starts: &[usize]) -> Option<usize> {
        let total = starts.len();
        let first = edit.start_row;
        let (old_end, new_end) = (edit.old_end_row, edit.new_end_row);
        let consistent = first <= old_end
            && first <= new_end
            && old_end < self.lines.len()
            && new_end < total
            && self.lines.len() - old_end + new_end == total;
        if !consistent {
            return None;
        }
        let mut state = self.lines[first].start.clone();
        let mut fresh = Vec::new();
        let mut row = first;
        while row < total {
            if row > new_end {
                let old = row - new_end + old_end;
                if self.lines.get(old).is_some_and(|line| line.start == state) {
                    let lexed = fresh.len();
                    self.lines.splice(first..old, fresh);
                    return Some(lexed);
                }
            }
            fresh.push(lex(&mut state, line_at(text, starts, row)));
            row += 1;
        }
        let lexed = fresh.len();
        self.lines.truncate(first);
        self.lines.extend(fresh);
        Some(lexed)
    }

    fn relex_all(&mut self, text: &str, starts: &[usize]) -> usize {
        let mut state = Highlighter::new(self.kind);
        self.lines = (0..starts.len())
            .map(|row| lex(&mut state, line_at(text, starts, row)))
            .collect();
        self.lines.len()
    }

    pub fn tokens(&self, row: usize) -> &[(usize, Token)] {
        self.lines.get(row).map_or(&[], |line| &line.tokens)
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn byte_range(&self, rows: Range<usize>) -> Range<usize> {
        let end = if rows.end >= self.lines.len() {
            self.text.len()
        } else {
            self.line_start(rows.end)
        };
        self.line_start(rows.start).min(end)..end
    }

    pub fn in_literal(&self, at: usize) -> bool {
        let at = at.min(self.text.len());
        if !self.text.is_char_boundary(at) {
            return false;
        }
        let row = self
            .starts
            .partition_point(|&start| start <= at)
            .saturating_sub(1);
        let Some(line) = self.lines.get(row) else {
            return false;
        };
        let mut probe = self.text[self.starts[row]..at].to_string();
        let col = probe.len();
        probe.push(' ');
        line.start
            .clone()
            .line(&probe)
            .into_iter()
            .find(|&(end, _)| end > col)
            .is_some_and(|(_, token)| {
                matches!(token, Token::String | Token::Comment | Token::Identifier)
            })
    }

    fn line_start(&self, row: usize) -> usize {
        self.starts.get(row).copied().unwrap_or(self.text.len())
    }

    pub fn styles(&self, range: &Range<usize>, palette: &Palette) -> Vec<(Range<usize>, Style)> {
        let mut out = Vec::new();
        let mut at = range.start;
        let first = self
            .starts
            .partition_point(|&start| start <= range.start.min(self.text.len()))
            .saturating_sub(1);
        for row in first..self.lines.len() {
            let line_start = self.starts[row];
            if line_start >= range.end {
                break;
            }
            let mut start = line_start;
            for &(end, token) in &self.lines[row].tokens {
                let end = line_start + end;
                let run = start.max(range.start)..end.min(range.end);
                if let Some(style) = palette.style(token).filter(|_| run.start < run.end) {
                    if at < run.start {
                        out.push((at..run.start, Style::default()));
                    }
                    at = run.end;
                    out.push((run, style));
                }
                start = end;
            }
        }
        if at < range.end {
            out.push((at..range.end, Style::default()));
        }
        out
    }

    pub fn spans(&self, rows: Range<usize>, palette: &Palette) -> Vec<Span> {
        let last = rows.end.min(self.lines.len());
        let first = rows.start.min(last);
        if first == last {
            return Vec::new();
        }
        let end = if last == self.lines.len() {
            self.text.len()
        } else {
            self.line_start(last)
        };
        let mut out = Vec::new();
        let mut open: Option<(Range<usize>, Style)> = None;
        for (run, style) in self.styles(&(self.line_start(first)..end), palette) {
            if style == Style::default() {
                continue;
            }
            match &mut open {
                Some((prev, prev_style)) if prev.end == run.start && *prev_style == style => {
                    prev.end = run.end;
                }
                _ => {
                    if let Some((prev, prev_style)) = open.replace((run, style)) {
                        out.push(prev_style.span(prev));
                    }
                }
            }
        }
        if let Some((prev, prev_style)) = open {
            out.push(prev_style.span(prev));
        }
        out
    }
}

fn lex(state: &mut Highlighter, line: &str) -> Line {
    let start = state.clone();
    Line {
        start,
        tokens: state.line(line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::DbKind::{MySql, Postgres, Sqlite};

    #[test]
    fn in_literal_tells_strings_and_comments_from_code() {
        let text = "select 'ab', x -- note\n/* open\n\nstill */ \"id\" 'cd";
        let mut lines = SqlLines::new(Sqlite);
        lines.sync(Sqlite, text);
        let at = |needle: &str| text.find(needle).unwrap();
        assert!(!lines.in_literal(0));
        assert!(!lines.in_literal(at("'ab'")), "before the quote is code");
        assert!(lines.in_literal(at("'ab'") + 2), "inside the string");
        assert!(!lines.in_literal(at("'ab'") + 4), "after the closed string");
        assert!(lines.in_literal(at("note")));
        assert!(
            lines.in_literal(at("-- note") + 7),
            "the end of a line comment"
        );
        assert!(lines.in_literal(at("open") + 4));
        assert!(
            lines.in_literal(at("\n\nstill") + 1),
            "an empty line in a block comment"
        );
        assert!(!lines.in_literal(at("*/") + 2));
        assert!(lines.in_literal(at("\"id\"") + 1));
        assert!(!lines.in_literal(at("\"id\"") + 4));
        assert!(
            lines.in_literal(text.len()),
            "an unclosed string runs to the end"
        );
    }

    fn row_of(text: &str, offset: usize) -> usize {
        text[..offset].matches('\n').count()
    }

    fn line_start_offset(text: &str, row: usize) -> usize {
        line_starts(text)[row]
    }

    fn edit(lines: &mut SqlLines, text: &mut String, range: Range<usize>, new: &str) -> usize {
        let old = text.clone();
        text.replace_range(range.clone(), new);
        let edit = LineEdit {
            start_row: row_of(&old, range.start),
            old_end_row: row_of(&old, range.end),
            new_end_row: row_of(text, range.start + new.len()),
        };
        lines.update(Some(&edit), text)
    }

    fn tokens(lines: &SqlLines, text: &str) -> Vec<(String, Token)> {
        let starts = line_starts(text);
        let mut out = Vec::new();
        for row in 0..lines.len() {
            let line = line_at(text, &starts, row);
            let mut start = 0;
            for &(end, token) in lines.tokens(row) {
                if token != Token::Plain {
                    out.push((line[start..end].to_string(), token));
                }
                start = end;
            }
        }
        out
    }

    fn fresh(kind: DbKind, text: &str) -> Vec<(String, Token)> {
        let mut lines = SqlLines::new(kind);
        lines.update(None, text);
        tokens(&lines, text)
    }

    fn of(found: &[(String, Token)], token: Token) -> Vec<String> {
        found
            .iter()
            .filter(|(_, t)| *t == token)
            .map(|(text, _)| text.clone())
            .collect()
    }

    #[test]
    fn the_dialect_decides_comments_strings_and_names() {
        let text = "SELECT 1 # note\n";
        assert_eq!(
            of(&fresh(MySql, text), Token::Comment),
            ["# note"],
            "MySQL # comment"
        );
        assert!(
            of(&fresh(Postgres, text), Token::Comment).is_empty(),
            "# is no comment in Postgres"
        );

        let text = "SELECT $$ a\nb $$, $tag$x$tag$";
        assert_eq!(
            of(&fresh(Postgres, text), Token::String),
            ["$$ a", "b $$", "$tag$x$tag$"]
        );
        assert!(of(&fresh(MySql, text), Token::String).is_empty());

        let text = "SELECT \"a\", [b] FROM t";
        assert_eq!(
            of(&fresh(MySql, text), Token::String),
            ["\"a\""],
            "MySQL double quotes are strings"
        );
        assert_eq!(of(&fresh(Postgres, text), Token::Identifier), ["\"a\""]);
        assert_eq!(
            of(&fresh(Sqlite, text), Token::Identifier),
            ["\"a\"", "[b]"]
        );
    }

    #[test]
    fn styles_cover_the_range_with_theme_styles() {
        let text = "select 'x' -- c\nfrom t";
        let mut lines = SqlLines::new(Sqlite);
        lines.update(None, text);
        let mut palette = Palette::default();
        let keyword = Style {
            bold: true,
            ..Default::default()
        };
        let comment = Style {
            italic: true,
            ..Default::default()
        };
        palette.styles[slot(Token::Keyword)] = Some(keyword);
        palette.styles[slot(Token::Comment)] = Some(comment);

        let all = lines.styles(&(0..text.len()), &palette);
        let runs: Vec<(&str, Style)> = all
            .iter()
            .map(|(range, style)| (&text[range.clone()], *style))
            .collect();
        let plain = Style::default();
        assert_eq!(
            runs,
            [
                ("select", keyword),
                (" 'x' ", plain),
                ("-- c", comment),
                ("\n", plain),
                ("from", keyword),
                (" t", plain)
            ]
        );
        assert_eq!(lines.styles(&(2..4), &palette), [(2..4, keyword)]);
        assert_eq!(
            lines.styles(&(16..22), &palette),
            [(16..20, keyword), (20..22, plain)]
        );
    }

    #[test]
    fn edits_relex_only_until_the_state_matches() {
        let mut text: String = (0..2000)
            .map(|i| format!("SELECT {i} FROM t; -- line {i}\n"))
            .collect();
        let mut lines = SqlLines::new(Postgres);
        assert_eq!(lines.update(None, &text), 2001);

        let at = line_start_offset(&text, 1000) + 7;
        assert_eq!(edit(&mut lines, &mut text, at..at, "42 + "), 1);
        assert_eq!(tokens(&lines, &text), fresh(Postgres, &text));

        let at = line_start_offset(&text, 10);
        let lexed = edit(&mut lines, &mut text, at..at, "/* ");
        assert_eq!(lexed, 1991);
        assert_eq!(tokens(&lines, &text), fresh(Postgres, &text));
        let at = line_start_offset(&text, 12);
        assert_eq!(edit(&mut lines, &mut text, at..at, "*/"), 1989);
        assert_eq!(tokens(&lines, &text), fresh(Postgres, &text));
        let at = line_start_offset(&text, 11) + 3;
        assert_eq!(edit(&mut lines, &mut text, at..at, "'"), 1);
        assert_eq!(tokens(&lines, &text), fresh(Postgres, &text));

        let at = line_start_offset(&text, 500);
        edit(&mut lines, &mut text, at - 1..at, "");
        assert_eq!(tokens(&lines, &text), fresh(Postgres, &text));
        let at = line_start_offset(&text, 600) + 3;
        edit(&mut lines, &mut text, at..at, "\n\n$$x\ny$$ ");
        assert_eq!(tokens(&lines, &text), fresh(Postgres, &text));
        let (from, to) = (
            line_start_offset(&text, 700) + 2,
            line_start_offset(&text, 705) + 4,
        );
        edit(&mut lines, &mut text, from..to, "");
        assert_eq!(tokens(&lines, &text), fresh(Postgres, &text));
        assert_eq!(lines.len(), line_starts(&text).len());

        let len = text.len();
        edit(&mut lines, &mut text, 0..len, "select 1");
        assert_eq!(tokens(&lines, &text), fresh(Postgres, &text));
        let bogus = LineEdit {
            start_row: 5,
            old_end_row: 9,
            new_end_row: 5,
        };
        assert_eq!(
            lines.update(Some(&bogus), &text),
            1,
            "re-lexed from the top"
        );
    }

    #[test]
    fn a_diff_of_the_texts_relexes_only_the_changed_lines() {
        let text: String = (0..300).map(|i| format!("SELECT {i};\n")).collect();
        let mut lines = SqlLines::new(Postgres);
        assert_eq!(lines.sync(Postgres, &text), 301);
        let edited = text.replacen("SELECT 150;", "SELECT 150 + 1;", 1);
        assert_eq!(lines.sync(Postgres, &edited), 1);
        assert_eq!(lines.sync(Postgres, &edited), 0, "no change, no lexing");
        assert_eq!(tokens(&lines, &edited), fresh(Postgres, &edited));
        let edited = edited.replacen("SELECT 20;\n", "", 1);
        lines.sync(Postgres, &edited);
        assert_eq!(tokens(&lines, &edited), fresh(Postgres, &edited));
        assert_eq!(lines.len(), 300);
        assert_eq!(lines.sync(Sqlite, &edited), 300, "another dialect re-lexes");
        let mut palette = Palette::default();
        palette.styles[slot(Token::Keyword)] = Some(Style {
            bold: true,
            ..Default::default()
        });
        let spans = lines.spans(10..12, &palette);
        assert_eq!(spans.len(), 2, "only the asked rows are painted");
        assert!(lines.spans(400..500, &palette).is_empty());
    }
}
