#[cfg(test)]
mod tests;

use crate::{
    connection::DbKind,
    db::{self, RowUpdate},
    highlight::{dollar_delimiter, is_word},
};

pub const PREVIEW_CHARS: usize = 120;

const UNKNOWN: &str = "UNKNOWN";
const UNTERMINATED: &str = "UNTERMINATED";

const SIDE_EFFECT_FUNCTIONS: &[&str] = &[
    "nextval",
    "setval",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_reload_conf",
    "pg_rotate_logfile",
    "pg_switch_wal",
    "pg_promote",
    "pg_create_restore_point",
    "pg_backup_start",
    "pg_backup_stop",
    "pg_start_backup",
    "pg_stop_backup",
    "pg_wal_replay_pause",
    "pg_wal_replay_resume",
    "pg_notify",
    "pg_logical_emit_message",
    "pg_import_system_collations",
    "pg_create_physical_replication_slot",
    "pg_create_logical_replication_slot",
    "pg_copy_physical_replication_slot",
    "pg_copy_logical_replication_slot",
    "pg_drop_replication_slot",
    "pg_replication_slot_advance",
    "pg_logical_slot_get_changes",
    "pg_logical_slot_get_binary_changes",
    "pg_replication_origin_create",
    "pg_replication_origin_drop",
    "pg_replication_origin_advance",
    "pg_replication_origin_session_setup",
    "pg_replication_origin_session_reset",
    "pg_replication_origin_xact_setup",
    "pg_replication_origin_xact_reset",
    "pg_stat_reset",
    "pg_stat_reset_shared",
    "pg_stat_reset_single_table_counters",
    "pg_stat_reset_single_function_counters",
    "pg_stat_reset_slru",
    "pg_stat_reset_replication_slot",
    "pg_stat_reset_subscription_stats",
    "pg_stat_reset_backend_stats",
    "pg_stat_statements_reset",
    "pg_file_write",
    "pg_file_rename",
    "pg_file_unlink",
    "pg_file_sync",
    "lo_import",
    "lo_export",
    "lo_unlink",
    "lo_create",
    "lo_creat",
    "lo_put",
    "lo_from_bytea",
    "lowrite",
    "lo_truncate",
    "lo_truncate64",
    "dblink",
    "dblink_exec",
    "dblink_send_query",
    "load_extension",
];

const PLAIN_READS: [&str; 17] = [
    "SHOW",
    "COMMIT",
    "END",
    "ROLLBACK",
    "ABORT",
    "SAVEPOINT",
    "RELEASE",
    "PREPARE",
    "DEALLOCATE",
    "FETCH",
    "MOVE",
    "CLOSE",
    "LISTEN",
    "UNLISTEN",
    "USE",
    "HELP",
    "UNLOCK",
];

const NESTED_STATEMENTS: [&str; 5] = ["INSERT", "UPDATE", "DELETE", "MERGE", "WITH"];

const TRANSACTION_WORDS: [&str; 16] = [
    "TRANSACTION",
    "WORK",
    "DEFERRED",
    "IMMEDIATE",
    "EXCLUSIVE",
    "ISOLATION",
    "LEVEL",
    "SERIALIZABLE",
    "REPEATABLE",
    "READ",
    "COMMITTED",
    "UNCOMMITTED",
    "WRITE",
    "ONLY",
    "NOT",
    "DEFERRABLE",
];

const WITH_MAIN: [&str; 8] = [
    "SELECT", "VALUES", "TABLE", "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE",
];

const EXPLAINED: [&str; 12] = [
    "SELECT", "VALUES", "TABLE", "WITH", "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE",
    "CREATE", "DECLARE", "EXECUTE",
];

const SET_WRITES: [&str; 4] = ["GLOBAL", "PERSIST", "PERSIST_ONLY", "PASSWORD"];

const PRAGMA_WRITES: [&str; 4] = [
    "optimize",
    "incremental_vacuum",
    "wal_checkpoint",
    "shrink_memory",
];

const PRAGMA_QUERIES: [&str; 11] = [
    "table_info",
    "table_xinfo",
    "table_list",
    "index_info",
    "index_xinfo",
    "index_list",
    "foreign_key_list",
    "foreign_key_check",
    "integrity_check",
    "quick_check",
    "function_list",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub verb: String,
    pub text: String,
    pub unbounded: bool,
}

pub fn writes(sql: &str, kind: DbKind) -> Vec<Statement> {
    let escapes = kind == DbKind::MySql;
    let mut found = scan(sql, kind, escapes);
    if kind == DbKind::Sqlite {
        return found;
    }
    let other = scan(sql, kind, !escapes);
    if other.iter().all(|stmt| stmt.verb != UNTERMINATED) {
        found.retain(|stmt| stmt.verb != UNTERMINATED);
    }
    for stmt in other.into_iter().filter(|stmt| stmt.verb != UNTERMINATED) {
        match found.iter_mut().find(|seen| seen.text == stmt.text) {
            Some(seen) => seen.unbounded |= stmt.unbounded,
            None => found.push(stmt),
        }
    }
    found
}

pub fn row_update(kind: DbKind, update: &RowUpdate) -> Result<Statement, String> {
    let sql = db::update_sql(kind, update, |value| {
        value.map_or_else(
            || "NULL".to_string(),
            |value| format!("'{}'", value.replace('\'', "''")),
        )
    })?;
    Ok(Statement {
        verb: "UPDATE".into(),
        text: preview(&sql),
        unbounded: false,
    })
}

fn scan(sql: &str, kind: DbKind, backslash: bool) -> Vec<Statement> {
    let lexer = Lexer {
        src: sql,
        kind,
        backslash,
    };
    lexer
        .pieces()
        .into_iter()
        .filter_map(|piece| {
            let verdict = classify(sql, kind, &piece.tokens)
                .or_else(|| piece.open.then(|| Verdict::write(UNTERMINATED)))?;
            let (first, last) = (piece.tokens.first()?, piece.tokens.last()?);
            Some(Statement {
                verb: verdict.verb,
                text: preview(&sql[first.start..last.end]),
                unbounded: verdict.unbounded,
            })
        })
        .collect()
}

fn preview(src: &str) -> String {
    let line = src
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(|c| if c.is_control() { '·' } else { c })
        .collect::<String>();
    if line.chars().count() <= PREVIEW_CHARS {
        return line;
    }
    let mut cut: String = line.chars().take(PREVIEW_CHARS - 1).collect();
    cut.push('…');
    cut
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Word,
    Quoted,
    Open,
    Close,
    Eq,
    Other,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    tok: Tok,
    depth: usize,
    start: usize,
    end: usize,
}

#[derive(Debug, Default)]
struct Piece {
    tokens: Vec<Token>,
    open: bool,
}

impl Piece {
    fn push(&mut self, tok: Tok, depth: usize, start: usize, end: usize) {
        self.tokens.push(Token {
            tok,
            depth,
            start,
            end,
        });
    }

    fn push_quoted(&mut self, depth: usize, start: usize, (end, closed): (usize, bool)) -> usize {
        self.push(Tok::Quoted, depth, start, end);
        self.open |= !closed;
        end
    }
}

struct Lexer<'a> {
    src: &'a str,
    kind: DbKind,
    backslash: bool,
}

impl Lexer<'_> {
    fn pieces(&self) -> Vec<Piece> {
        let bytes = self.src.as_bytes();
        let mut pieces = Vec::new();
        let mut piece = Piece::default();
        let mut depth = 0usize;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            let next = bytes.get(i + 1).copied();
            match b {
                _ if b.is_ascii_whitespace() => i += 1,
                b';' => {
                    if !piece.tokens.is_empty() {
                        pieces.push(std::mem::take(&mut piece));
                    }
                    depth = 0;
                    i += 1;
                }
                b'-' if next == Some(b'-') && self.line_comment(bytes.get(i + 2).copied()) => {
                    i = self.line_end(bytes, i);
                }
                b'#' if self.kind == DbKind::MySql => i = self.line_end(bytes, i),
                b'/' if next == Some(b'*') => match self.executable_comment(bytes, i) {
                    Some(end) => i = end,
                    None => match self.block_comment_end(bytes, i) {
                        Some(end) => i = end,
                        None => {
                            piece.open = true;
                            i = bytes.len();
                        }
                    },
                },
                b'*' if next == Some(b'/') => i += 2,
                b'\'' => {
                    let mut start = i;
                    let mut escapes = self.backslash;
                    if self.kind == DbKind::Postgres
                        && let Some(prev) = piece.tokens.last()
                        && prev.tok == Tok::Word
                        && prev.end == i
                        && self.src[prev.start..prev.end].eq_ignore_ascii_case("e")
                    {
                        escapes = true;
                        start = prev.start;
                        piece.tokens.pop();
                    }
                    i = piece.push_quoted(depth, start, quoted_end(bytes, i, b'\'', escapes));
                }
                b'"' => {
                    let escapes = self.kind == DbKind::MySql && self.backslash;
                    i = piece.push_quoted(depth, i, quoted_end(bytes, i, b'"', escapes));
                }
                b'`' if self.kind != DbKind::Postgres => {
                    i = piece.push_quoted(depth, i, quoted_end(bytes, i, b'`', false));
                }
                b'[' if self.kind == DbKind::Sqlite => {
                    let end = match bytes[i + 1..].iter().position(|&c| c == b']') {
                        Some(at) => (i + at + 2, true),
                        None => (bytes.len(), false),
                    };
                    i = piece.push_quoted(depth, i, end);
                }
                b'$' if let Some(tag) = self.dollar_quote(bytes, i) => {
                    let delimiter = &bytes[i..i + tag];
                    let body = i + tag;
                    let end = match bytes[body..]
                        .windows(tag)
                        .position(|window| window == delimiter)
                    {
                        Some(at) => (body + at + tag, true),
                        None => (bytes.len(), false),
                    };
                    i = piece.push_quoted(depth, i, end);
                }
                b'(' => {
                    piece.push(Tok::Open, depth, i, i + 1);
                    depth += 1;
                    i += 1;
                }
                b')' => {
                    depth = depth.saturating_sub(1);
                    piece.push(Tok::Close, depth, i, i + 1);
                    i += 1;
                }
                b'=' => {
                    piece.push(Tok::Eq, depth, i, i + 1);
                    i += 1;
                }
                _ if b.is_ascii_alphabetic() || b == b'_' || !b.is_ascii() => {
                    let end = run_end(bytes, i, |c| is_word(c) || c == b'$');
                    piece.push(Tok::Word, depth, i, end);
                    i = end;
                }
                _ if b.is_ascii_digit() => {
                    let end = run_end(bytes, i, is_word);
                    piece.push(Tok::Other, depth, i, end);
                    i = end;
                }
                _ => {
                    piece.push(Tok::Other, depth, i, i + 1);
                    i += 1;
                }
            }
        }
        if !piece.tokens.is_empty() {
            pieces.push(piece);
        }
        pieces
    }

    fn dollar_quote(&self, bytes: &[u8], at: usize) -> Option<usize> {
        let after_name = at > 0 && (is_word(bytes[at - 1]) || bytes[at - 1] == b'$');
        if self.kind != DbKind::Postgres || after_name {
            return None;
        }
        dollar_delimiter(&bytes[at..])
    }

    fn line_comment(&self, after: Option<u8>) -> bool {
        self.kind != DbKind::MySql || after.is_none_or(|c| c.is_ascii_whitespace() || c < b' ')
    }

    fn line_end(&self, bytes: &[u8], at: usize) -> usize {
        let cr = self.kind == DbKind::Postgres;
        bytes[at..]
            .iter()
            .position(|&c| c == b'\n' || (cr && c == b'\r'))
            .map_or(bytes.len(), |n| at + n + 1)
    }

    fn executable_comment(&self, bytes: &[u8], at: usize) -> Option<usize> {
        if self.kind != DbKind::MySql {
            return None;
        }
        let rest = &bytes[at + 2..];
        let opener = if rest.starts_with(b"!") {
            3
        } else if rest.starts_with(b"M!") {
            4
        } else {
            return None;
        };
        Some(run_end(bytes, at + opener, |c| c.is_ascii_digit()))
    }

    fn block_comment_end(&self, bytes: &[u8], at: usize) -> Option<usize> {
        let nests = self.kind == DbKind::Postgres;
        let mut level = 1;
        let mut i = at + 2;
        while i + 1 < bytes.len() {
            match (bytes[i], bytes[i + 1]) {
                (b'*', b'/') => {
                    level -= 1;
                    i += 2;
                    if level == 0 {
                        return Some(i);
                    }
                }
                (b'/', b'*') if nests => {
                    level += 1;
                    i += 2;
                }
                _ => i += 1,
            }
        }
        None
    }
}

fn run_end(bytes: &[u8], at: usize, keep: impl Fn(u8) -> bool) -> usize {
    at + bytes[at..].iter().take_while(|&&c| keep(c)).count()
}

fn quoted_end(bytes: &[u8], at: usize, quote: u8, escapes: bool) -> (usize, bool) {
    let mut i = at + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if escapes => i += 2,
            c if c == quote && bytes.get(i + 1) == Some(&quote) => i += 2,
            c if c == quote => return (i + 1, true),
            _ => i += 1,
        }
    }
    (bytes.len(), false)
}

struct Verdict {
    verb: String,
    unbounded: bool,
}

impl Verdict {
    fn write(verb: impl Into<String>) -> Self {
        Self {
            verb: verb.into(),
            unbounded: false,
        }
    }
}

fn word<'a>(src: &'a str, token: &Token) -> Option<&'a str> {
    (token.tok == Tok::Word).then(|| &src[token.start..token.end])
}

fn is(src: &str, token: &Token, expected: &str) -> bool {
    word(src, token).is_some_and(|w| w.eq_ignore_ascii_case(expected))
}

fn upper(src: &str, token: &Token) -> Option<String> {
    word(src, token).map(str::to_ascii_uppercase)
}

fn function_name<'a>(src: &'a str, token: &Token) -> Option<&'a str> {
    let text = &src[token.start..token.end];
    match token.tok {
        Tok::Word => Some(text),
        Tok::Quoted if text.len() >= 2 && matches!(text.as_bytes()[0], b'"' | b'`' | b'[') => {
            Some(&text[1..text.len() - 1])
        }
        _ => None,
    }
}

fn classify(src: &str, kind: DbKind, toks: &[Token]) -> Option<Verdict> {
    let start = toks.iter().position(|t| t.tok != Tok::Open);
    let Some((rest, verb)) = start.and_then(|at| Some((&toks[at..], upper(src, &toks[at])?)))
    else {
        return Some(Verdict::write(UNKNOWN));
    };
    let own = match verb.as_str() {
        "EXPLAIN" | "DESCRIBE" | "DESC" => return explain(src, kind, rest),
        "SELECT" | "VALUES" | "TABLE" => select(src, toks, rest),
        "WITH" => with(src, toks, rest),
        "SET" => set(src, rest).or_else(|| calls(src, toks, "SET")),
        "DECLARE" => calls(src, toks, "DECLARE"),
        "BEGIN" => begin(src, kind, rest),
        "START" => {
            let transaction = rest.get(1).is_some_and(|t| is(src, t, "TRANSACTION"));
            (!transaction).then(|| Verdict::write(verb.as_str()))
        }
        "RESET" => (kind != DbKind::Postgres).then(|| Verdict::write(verb.as_str())),
        "PRAGMA" => pragma(src, rest),
        _ if PLAIN_READS.contains(&verb.as_str()) => None,
        _ => Some(Verdict {
            unbounded: unbounded(src, rest),
            verb,
        }),
    };
    let nested = nested_writes(src, kind, toks);
    let any_unbounded = nested.iter().any(|v| v.unbounded);
    match own {
        Some(mut own) => {
            own.unbounded |= any_unbounded;
            Some(own)
        }
        None => nested.first().map(|inner| {
            let head = format!("{} … ", upper(src, &rest[0]).unwrap_or_default());
            let verb = if inner.verb.starts_with(&head) {
                inner.verb.clone()
            } else {
                format!("{head}{}", inner.verb)
            };
            Verdict {
                verb,
                unbounded: any_unbounded,
            }
        }),
    }
}

fn unbounded(src: &str, stmt: &[Token]) -> bool {
    let verb = &stmt[0];
    let changes = is(src, verb, "UPDATE") || is(src, verb, "DELETE");
    changes
        && !stmt
            .iter()
            .any(|t| t.depth == verb.depth && is(src, t, "WHERE"))
}

fn nested_writes(src: &str, kind: DbKind, toks: &[Token]) -> Vec<Verdict> {
    let mut found = Vec::new();
    let mut i = 0;
    while i + 1 < toks.len() {
        let open = &toks[i];
        let starts_statement = open.tok == Tok::Open
            && word(src, &toks[i + 1])
                .is_some_and(|w| NESTED_STATEMENTS.iter().any(|v| v.eq_ignore_ascii_case(w)));
        if !starts_statement {
            i += 1;
            continue;
        }
        let close = toks[i + 1..]
            .iter()
            .position(|t| t.tok == Tok::Close && t.depth == open.depth)
            .map_or(toks.len(), |n| i + 1 + n);
        found.extend(classify(src, kind, &toks[i + 1..close]));
        i = close;
    }
    found
}

fn select(src: &str, all: &[Token], stmt: &[Token]) -> Option<Verdict> {
    let level = stmt[0].depth;
    if stmt.iter().any(|t| t.depth <= level && is(src, t, "INTO")) {
        return Some(Verdict::write("SELECT … INTO"));
    }
    calls(src, all, "SELECT")
}

fn calls(src: &str, all: &[Token], verb: &str) -> Option<Verdict> {
    all.windows(2).find_map(|pair| {
        let name = function_name(src, &pair[0])?;
        let function = SIDE_EFFECT_FUNCTIONS
            .iter()
            .find(|f| f.eq_ignore_ascii_case(name))?;
        (pair[1].tok == Tok::Open).then(|| Verdict::write(format!("{verb} {function}()")))
    })
}

fn begin(src: &str, kind: DbKind, stmt: &[Token]) -> Option<Verdict> {
    let rest = &stmt[1..];
    let transaction = rest.iter().all(|t| {
        upper(src, t).is_some_and(|w| TRANSACTION_WORDS.contains(&w.as_str()))
            || &src[t.start..t.end] == ","
    });
    if transaction {
        return None;
    }
    let block = rest.len() >= 2 && is(src, &rest[0], "NOT") && is(src, &rest[1], "ATOMIC");
    if !block {
        return Some(Verdict::write("BEGIN"));
    }
    let inner = &rest[2..];
    if inner.is_empty() {
        return None;
    }
    classify(src, kind, inner).map(|v| Verdict {
        verb: format!("BEGIN NOT ATOMIC {}", v.verb),
        unbounded: v.unbounded,
    })
}

fn with(src: &str, all: &[Token], stmt: &[Token]) -> Option<Verdict> {
    let level = stmt[0].depth;
    let Some((at, main)) = stmt.iter().enumerate().skip(1).find_map(|(at, t)| {
        let name = upper(src, t).filter(|w| t.depth == level && WITH_MAIN.contains(&w.as_str()))?;
        Some((at, name))
    }) else {
        return Some(Verdict::write("WITH"));
    };
    match main.as_str() {
        "SELECT" | "VALUES" | "TABLE" => select(src, all, &stmt[at..]),
        _ => Some(Verdict {
            unbounded: unbounded(src, &stmt[at..]),
            verb: format!("WITH … {main}"),
        }),
    }
}

fn explain(src: &str, kind: DbKind, stmt: &[Token]) -> Option<Verdict> {
    let level = stmt[0].depth;
    let mut analyze = false;
    let mut inner = None;
    for (at, t) in stmt.iter().enumerate().skip(1) {
        let Some(w) = upper(src, t) else {
            continue;
        };
        if t.depth == level && EXPLAINED.contains(&w.as_str()) {
            inner = Some(at);
            break;
        }
        analyze |= w == "ANALYZE" || w == "ANALYSE";
    }
    if !analyze {
        return None;
    }
    let Some(at) = inner else {
        return Some(Verdict::write("EXPLAIN ANALYZE"));
    };
    classify(src, kind, &stmt[at..]).map(|v| Verdict {
        verb: format!("EXPLAIN ANALYZE {}", v.verb),
        unbounded: v.unbounded,
    })
}

fn set(src: &str, stmt: &[Token]) -> Option<Verdict> {
    if let Some(w) = stmt
        .iter()
        .filter_map(|t| upper(src, t))
        .find(|w| SET_WRITES.contains(&w.as_str()))
    {
        return Some(Verdict::write(format!("SET {w}")));
    }
    stmt.get(1)
        .is_some_and(|t| is(src, t, "DEFAULT"))
        .then(|| Verdict::write("SET DEFAULT"))
}

fn pragma(src: &str, stmt: &[Token]) -> Option<Verdict> {
    let write = || Some(Verdict::write("PRAGMA"));
    if stmt.iter().any(|t| t.tok == Tok::Eq) {
        return write();
    }
    let head = stmt
        .iter()
        .position(|t| t.tok == Tok::Open)
        .unwrap_or(stmt.len());
    let name = stmt[..head].iter().rev().find_map(|t| word(src, t))?;
    let acts = PRAGMA_WRITES.iter().any(|p| p.eq_ignore_ascii_case(name));
    let sets = head < stmt.len() && !PRAGMA_QUERIES.iter().any(|p| p.eq_ignore_ascii_case(name));
    if acts || sets { write() } else { None }
}
