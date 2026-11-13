use crate::connection::DbKind;

mod words;

const BODY_KEYWORDS: &[&str] = &[
    "ASSERT",
    "CONSTANT",
    "CONTINUE",
    "ELSEIF",
    "ELSIF",
    "EXCEPTION",
    "EXIT",
    "FOREACH",
    "LOOP",
    "NEXT",
    "PERFORM",
    "QUERY",
    "RAISE",
    "STRICT",
    "WHILE",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Plain,
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Identifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    No,
    Hard,
    Soft,
    Type,
    Fixed,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Place {
    #[default]
    Start,
    Operand,
    Name,
    After,
    Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Alter,
    Set,
    Define,
    Grant,
    Lock,
    Other,
}

impl Verb {
    fn of(name: &str) -> Self {
        match name {
            "ALTER" => Verb::Alter,
            "SET" => Verb::Set,
            "CREATE" | "DROP" => Verb::Define,
            "GRANT" | "REVOKE" | "COMMENT" => Verb::Grant,
            "LOCK" => Verb::Lock,
            _ => Verb::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Paren {
    Plain,
    Cast,
    Signature,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
enum State {
    #[default]
    Code,
    Comment(usize),
    Quote {
        close: u8,
        escapes: bool,
        token: Token,
    },
    Dollar(String),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Lead {
    #[default]
    Other,
    Body,
    Language,
    LanguageName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlighter {
    kind: DbKind,
    state: State,
    lead: Lead,
    bodies: Vec<String>,
    place: Place,
    keyword: Option<&'static str>,
    verb: Option<Verb>,
    parens: Vec<Paren>,
    signature: bool,
}

impl Highlighter {
    pub fn new(kind: DbKind) -> Self {
        Self {
            kind,
            state: State::Code,
            lead: Lead::Other,
            bodies: Vec::new(),
            place: Place::Start,
            keyword: None,
            verb: None,
            parens: Vec::new(),
            signature: false,
        }
    }

    pub fn line(&mut self, line: &str) -> Vec<(usize, Token)> {
        let mut tokens: Vec<(usize, Token)> = Vec::new();
        let mut push = |end: usize, token: Token| match tokens.last_mut() {
            Some(last) if last.1 == token => last.0 = end,
            _ => tokens.push((end, token)),
        };
        let mut at = 0;
        while at < line.len() {
            let close = self.body_close(line, at);
            let end = close.map_or(line.len(), |(found, _)| found);
            let depth = self.bodies.len();
            while at < end && self.bodies.len() == depth {
                let (next, token) = self.step(&line[..end], at);
                push(next, token);
                at = next;
            }
            if let Some((found, body)) = close
                && at == found
            {
                at += self.bodies[body].len();
                push(at, Token::String);
                self.bodies.truncate(body);
                self.state = State::Code;
                self.lead = Lead::Other;
                self.parens.clear();
                self.value();
            }
        }
        tokens
    }

    fn body_close(&self, line: &str, at: usize) -> Option<(usize, usize)> {
        let found = |(body, delimiter): (usize, &String)| {
            line[at..].find(delimiter).map(|found| (at + found, body))
        };
        self.bodies.iter().enumerate().filter_map(found).min()
    }

    fn step(&mut self, line: &str, at: usize) -> (usize, Token) {
        match std::mem::take(&mut self.state) {
            State::Code => self.code(line, at),
            State::Comment(depth) => self.comment(line, at, depth),
            State::Quote {
                close,
                escapes,
                token,
            } => self.quote(line, at, close, escapes, token),
            State::Dollar(delimiter) => self.dollar(line, at, delimiter),
        }
    }

    fn code(&mut self, line: &str, at: usize) -> (usize, Token) {
        let bytes = line.as_bytes();
        let rest = &bytes[at..];
        let kind = self.kind;
        let follows = |byte: fn(u8) -> bool| at > 0 && byte(bytes[at - 1]);
        let before = |byte: fn(&u8) -> bool| rest.get(1).is_some_and(byte);
        let lead = std::mem::take(&mut self.lead);
        let mut skip = |end: usize, token: Token| {
            self.lead = lead;
            (end, token)
        };
        match rest[0] {
            b'-' if rest.starts_with(b"--")
                && (kind != DbKind::MySql || rest.get(2).is_none_or(u8::is_ascii_whitespace)) =>
            {
                skip(bytes.len(), Token::Comment)
            }
            b'#' if kind == DbKind::MySql => skip(bytes.len(), Token::Comment),
            b'/' if rest.starts_with(b"/*") => {
                self.lead = lead;
                self.comment(line, at + 2, 1)
            }
            b if b.is_ascii_whitespace() => skip(at + 1, Token::Plain),
            b'\'' => self.quoted(line, at + 1, b'\'', kind == DbKind::MySql, Token::String),
            b'"' if kind == DbKind::MySql => self.quoted(line, at + 1, b'"', true, Token::String),
            b'"' => self.quoted(line, at + 1, b'"', false, Token::Identifier),
            b'`' if kind != DbKind::Postgres => {
                self.quoted(line, at + 1, b'`', false, Token::Identifier)
            }
            b'[' if kind == DbKind::Sqlite => {
                self.quoted(line, at + 1, b']', false, Token::Identifier)
            }
            b'$' if kind == DbKind::Postgres && dollar_delimiter(rest).is_some() => {
                let end = at + dollar_delimiter(rest).unwrap_or(1);
                let delimiter = line[at..end].to_string();
                if matches!(lead, Lead::Body | Lead::LanguageName) {
                    self.bodies.push(delimiter);
                    self.begin();
                    (end, Token::String)
                } else {
                    self.value();
                    self.dollar(line, end, delimiter)
                }
            }
            b'$' | b'@' | b':' if before(|&b| is_word(b)) && !follows(|b| b == b':') => {
                self.value();
                (word_end(bytes, at + 1), Token::Plain)
            }
            b'0'..=b'9' => {
                self.value();
                (number(bytes, at), Token::Number)
            }
            b'.' if before(u8::is_ascii_digit) && !follows(|b| is_word(b) || b == b'.') => {
                self.value();
                (number(bytes, at), Token::Number)
            }
            b if is_word(b) => self.word(line, at, lead),
            _ => (at + self.punctuation(rest), Token::Plain),
        }
    }

    fn word(&mut self, line: &str, at: usize, lead: Lead) -> (usize, Token) {
        let bytes = line.as_bytes();
        let end = word_end(bytes, at);
        let word = &line[at..end];
        let escape_string = self.kind == DbKind::Postgres
            && word.eq_ignore_ascii_case("e")
            && bytes.get(end) == Some(&b'\'');
        if escape_string {
            self.value();
            return self.quote(line, end + 1, b'\'', true, Token::String);
        }
        let rest = line[end..].trim_start();
        let call = rest.starts_with('(');
        let function = call && self.place == Place::Operand;
        let qualifies = bytes.get(end) == Some(&b'.');
        if at > 0 && bytes[at - 1] == b'.' {
            if !qualifies {
                self.value();
            }
            let token = if function {
                Token::Function
            } else {
                Token::Plain
            };
            return (end, token);
        }
        if qualifies {
            return (end, Token::Plain);
        }
        let is = |keyword: &str| word.eq_ignore_ascii_case(keyword);
        self.lead = match lead {
            _ if is("as") || is("do") => Lead::Body,
            _ if is("language") => Lead::Language,
            Lead::Language => Lead::LanguageName,
            _ => Lead::Other,
        };
        let in_body = !self.bodies.is_empty();
        let (name, class) = match find(BODY_KEYWORDS, word) {
            Some(name) if in_body => (name, Class::Hard),
            _ => class(self.kind, word).unwrap_or(("", Class::No)),
        };
        match self.verb {
            None => self.verb = Some(Verb::of(name)),
            Some(Verb::Define | Verb::Alter) if name == "SELECT" => self.verb = Some(Verb::Other),
            _ => {}
        }
        let next = &rest[..word_end(rest.as_bytes(), 0)];
        let paren = self.parens.last();
        let alias = self.keyword == Some("AS") && paren != Some(&Paren::Cast) && !follows_as(name);
        let paired = paired(self.keyword, name, next, call);
        let free = matches!(self.place, Place::Start | Place::After);
        let typed = self.place == Place::After
            || paren == Some(&Paren::Signature)
            || rest.starts_with('\'');
        let token = match class {
            Class::Hard | Class::Fixed if function && callable(self.kind, name) => Token::Function,
            Class::Hard if alias => Token::Plain,
            Class::Hard => Token::Keyword,
            _ if self.place == Place::Type => Token::Type,
            Class::Fixed => Token::Type,
            Class::Soft if (free || paired) && !alias => Token::Keyword,
            Class::Type if paired => Token::Keyword,
            Class::Type if typed => Token::Type,
            _ if function => Token::Function,
            _ => Token::Plain,
        };
        if token == Token::Keyword {
            self.follow(name, in_body);
        } else {
            self.value();
        }
        (end, token)
    }

    fn follow(&mut self, name: &'static str, in_body: bool) {
        let previous = self.keyword.replace(name);
        let cast = self.parens.last() == Some(&Paren::Cast);
        self.signature |= matches!(name, "FUNCTION" | "PROCEDURE" | "AGGREGATE");
        self.place = match name {
            "AS" if cast => Place::Type,
            "AS" => Place::Name,
            "SET" if matches!(self.verb, Some(Verb::Alter | Verb::Set)) => Place::After,
            "SET" => Place::Operand,
            "ON" | "IN" if matches!(self.verb, Some(Verb::Grant | Verb::Lock)) => Place::After,
            "ON" if matches!(self.verb, Some(Verb::Define | Verb::Alter)) => Place::Name,
            "NOT" if previous == Some("IS") => Place::After,
            "TYPE" if matches!(previous, Some("CREATE" | "ALTER" | "DROP" | "ON")) => Place::Name,
            "TYPE" | "RETURNS" => Place::Type,
            "THEN" | "ELSE" | "LOOP" | "BEGIN" if in_body => {
                self.verb = None;
                Place::Start
            }
            _ if expects_operand(name) => Place::Operand,
            _ if expects_name(name) => Place::Name,
            _ => Place::After,
        };
    }

    fn punctuation(&mut self, rest: &[u8]) -> usize {
        if rest[0] == b'.' {
            return 1;
        }
        let keyword = self.keyword.take();
        let cast = self.kind == DbKind::Postgres && rest.starts_with(b"::");
        self.place = match rest[0] {
            b';' => {
                self.begin();
                Place::Start
            }
            b'(' => {
                let signature = std::mem::take(&mut self.signature);
                self.parens.push(match keyword {
                    Some("CAST") => Paren::Cast,
                    _ if signature => Paren::Signature,
                    _ => Paren::Plain,
                });
                Place::Operand
            }
            b')' => {
                self.parens.pop();
                Place::After
            }
            b']' => Place::After,
            b',' if self.verb == Some(Verb::Alter) && self.parens.is_empty() => Place::After,
            b'*' if self.place != Place::After && self.place != Place::Type => Place::After,
            b':' if cast => Place::Type,
            b':' if rest.get(1) != Some(&b'=') => Place::After,
            _ => Place::Operand,
        };
        1 + usize::from(cast)
    }

    fn begin(&mut self) {
        self.place = Place::Start;
        self.keyword = None;
        self.verb = None;
        self.parens.clear();
        self.signature = false;
    }

    fn value(&mut self) {
        self.place = Place::After;
        self.keyword = None;
    }

    fn quoted(
        &mut self,
        line: &str,
        at: usize,
        close: u8,
        escapes: bool,
        token: Token,
    ) -> (usize, Token) {
        self.value();
        self.quote(line, at, close, escapes, token)
    }

    fn comment(&mut self, line: &str, at: usize, mut depth: usize) -> (usize, Token) {
        let bytes = line.as_bytes();
        let nests = self.kind == DbKind::Postgres;
        let mut i = at;
        while i < bytes.len() {
            if bytes[i..].starts_with(b"*/") {
                i += 2;
                depth -= 1;
                if depth == 0 {
                    return (i, Token::Comment);
                }
            } else if nests && bytes[i..].starts_with(b"/*") {
                i += 2;
                depth += 1;
            } else {
                i += 1;
            }
        }
        self.state = State::Comment(depth);
        (bytes.len(), Token::Comment)
    }

    fn quote(
        &mut self,
        line: &str,
        at: usize,
        close: u8,
        escapes: bool,
        token: Token,
    ) -> (usize, Token) {
        let bytes = line.as_bytes();
        let doubles = close != b']';
        let mut i = at;
        while i < bytes.len() {
            if escapes && bytes[i] == b'\\' {
                i += 2;
            } else if bytes[i] != close {
                i += 1;
            } else if doubles && bytes.get(i + 1) == Some(&close) {
                i += 2;
            } else {
                return (i + 1, token);
            }
        }
        self.state = State::Quote {
            close,
            escapes,
            token,
        };
        (bytes.len(), token)
    }

    fn dollar(&mut self, line: &str, at: usize, delimiter: String) -> (usize, Token) {
        match line[at..].find(&delimiter) {
            Some(found) => (at + found + delimiter.len(), Token::String),
            None => {
                self.state = State::Dollar(delimiter);
                (line.len(), Token::String)
            }
        }
    }
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii()
}

fn word_end(bytes: &[u8], at: usize) -> usize {
    let word = bytes[at..]
        .iter()
        .take_while(|&&b| is_word(b) || b == b'$')
        .count();
    at + word
}

fn search<T>(table: &[T], name: fn(&T) -> &'static str, word: &str) -> Option<usize> {
    let upper = word.bytes().map(|b| b.to_ascii_uppercase());
    table
        .binary_search_by(|entry| name(entry).bytes().cmp(upper.clone()))
        .ok()
}

fn find(keywords: &[&'static str], word: &str) -> Option<&'static str> {
    search(keywords, |keyword| keyword, word).map(|found| keywords[found])
}

fn class(kind: DbKind, word: &str) -> Option<(&'static str, Class)> {
    let (name, classes) = words::WORDS[search(words::WORDS, |entry| entry.0, word)?];
    let class = match kind {
        DbKind::Postgres => classes[0],
        DbKind::MySql => classes[1],
        DbKind::Sqlite => classes[2],
    };
    (class != Class::No).then_some((name, class))
}

fn expects_operand(name: &str) -> bool {
    matches!(
        name,
        "AND"
            | "ANY"
            | "ASSERT"
            | "BETWEEN"
            | "BY"
            | "CALL"
            | "CASE"
            | "CHECK"
            | "DEFAULT"
            | "DISTINCT"
            | "DIV"
            | "ELSE"
            | "ELSEIF"
            | "ELSIF"
            | "ESCAPE"
            | "EXCEPT"
            | "FROM"
            | "FUNCTION"
            | "GLOB"
            | "HAVING"
            | "IF"
            | "ILIKE"
            | "IN"
            | "INTERSECT"
            | "INTERVAL"
            | "JOIN"
            | "LATERAL"
            | "LIKE"
            | "LIMIT"
            | "MATCH"
            | "MOD"
            | "NOT"
            | "OFFSET"
            | "ON"
            | "ONLY"
            | "OR"
            | "PERFORM"
            | "PROCEDURE"
            | "REGEXP"
            | "RETURN"
            | "RETURNING"
            | "RLIKE"
            | "SELECT"
            | "SOME"
            | "STRAIGHT_JOIN"
            | "THEN"
            | "UNION"
            | "VALUES"
            | "WHEN"
            | "WHERE"
            | "WHILE"
            | "XOR"
    )
}

fn expects_name(name: &str) -> bool {
    matches!(
        name,
        "ADD"
            | "AFTER"
            | "ANALYZE"
            | "CHANGE"
            | "COLLATE"
            | "COLUMN"
            | "CONSTRAINT"
            | "DATABASE"
            | "DECLARE"
            | "DESCRIBE"
            | "DOMAIN"
            | "EXISTS"
            | "EXTENSION"
            | "INDEX"
            | "INTO"
            | "KEY"
            | "LANGUAGE"
            | "MODIFY"
            | "OVER"
            | "POLICY"
            | "PRAGMA"
            | "PUBLICATION"
            | "RECURSIVE"
            | "REFERENCES"
            | "REINDEX"
            | "RELEASE"
            | "RENAME"
            | "ROLE"
            | "SAVEPOINT"
            | "SCHEMA"
            | "SEQUENCE"
            | "TABLE"
            | "TABLESPACE"
            | "TO"
            | "TRIGGER"
            | "TRUNCATE"
            | "UPDATE"
            | "USE"
            | "USER"
            | "USING"
            | "VACUUM"
            | "VIEW"
            | "WINDOW"
            | "WITH"
    )
}

fn follows_as(name: &str) -> bool {
    matches!(
        name,
        "SELECT"
            | "WITH"
            | "VALUES"
            | "TABLE"
            | "MATERIALIZED"
            | "NOT"
            | "ENUM"
            | "RANGE"
            | "IDENTITY"
            | "EXECUTE"
            | "ON"
    )
}

fn callable(kind: DbKind, name: &str) -> bool {
    match kind {
        DbKind::Postgres | DbKind::Sqlite => matches!(name, "LEFT" | "RIGHT"),
        DbKind::MySql => matches!(
            name,
            "LEFT"
                | "RIGHT"
                | "REPLACE"
                | "IF"
                | "INSERT"
                | "MOD"
                | "REPEAT"
                | "CONVERT"
                | "CHAR"
                | "VALUES"
                | "DATABASE"
                | "SCHEMA"
        ),
    }
}

fn paired(previous: Option<&str>, name: &str, next: &str, call: bool) -> bool {
    let next = |words: &[&str]| words.iter().any(|word| next.eq_ignore_ascii_case(word));
    let ahead = match name {
        "PARTITION" => next(&["BY"]),
        "UNBOUNDED" => next(&["PRECEDING", "FOLLOWING"]),
        "CURRENT" => next(&["ROW"]),
        "IF" => next(&["NOT", "EXISTS"]),
        "SKIP" => next(&["LOCKED"]),
        "NO" => next(&["ACTION", "DATA"]),
        "EXCLUDE" => next(&["USING"]),
        "GROUPING" => next(&["SETS"]),
        "TEMP" => next(&["TABLE", "VIEW"]),
        "MATERIALIZED" => next(&["VIEW"]),
        "LESS" => next(&["THAN"]),
        "TIME" => next(&["ZONE"]),
        "ROWS" | "RANGE" | "GROUPS" => next(&["BETWEEN", "UNBOUNDED", "CURRENT"]),
        "ROLLUP" | "CUBE" => call,
        _ => false,
    };
    let behind = match (previous, name) {
        (Some("ON"), "CONFLICT" | "DUPLICATE" | "COMMIT")
        | (
            Some("NOT"),
            "MATERIALIZED" | "VALID" | "INDEXED" | "MATCHED" | "LIKE" | "GLOB" | "REGEXP" | "MATCH",
        )
        | (Some("WITH"), "ROLLUP" | "ORDINALITY" | "TIES" | "DATA" | "NO" | "LOCAL" | "CASCADED")
        | (Some("OR"), "REPLACE" | "IGNORE" | "ROLLBACK" | "ABORT" | "FAIL")
        | (Some("TO"), "SAVEPOINT" | "STDOUT")
        | (Some("FROM"), "STDIN")
        | (Some("IN"), "EXECUTE")
        | (Some("WHEN"), "MATCHED")
        | (Some("DEFAULT"), "CHARSET")
        | (Some("CHECK"), "OPTION")
        | (Some("TABLE"), "STATUS") => true,
        (Some("BY"), "RANGE" | "LIST" | "HASH") => call,
        (Some("AS"), _) => follows_as(name),
        _ => false,
    };
    ahead || behind
}

fn dollar_delimiter(rest: &[u8]) -> Option<usize> {
    let tag = rest[1..].iter().take_while(|&&b| is_word(b)).count();
    let numbered = rest.get(1).is_some_and(u8::is_ascii_digit);
    (!numbered && rest.get(1 + tag) == Some(&b'$')).then_some(tag + 2)
}

fn number(bytes: &[u8], at: usize) -> usize {
    let run = |mut i: usize, digit: fn(&u8) -> bool| {
        while bytes.get(i).is_some_and(|b| digit(b) || *b == b'_') {
            i += 1;
        }
        i
    };
    let mut i = run(at, u8::is_ascii_digit);
    let radix = matches!(bytes.get(i), Some(b'x' | b'X' | b'o' | b'O' | b'b' | b'B'));
    if radix && &bytes[at..i] == b"0" {
        return run(i + 1, u8::is_ascii_hexdigit);
    }
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1) != Some(&b'.') {
        i = run(i + 1, u8::is_ascii_digit);
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(i + 1), Some(b'+' | b'-')));
        if bytes.get(i + 1 + sign).is_some_and(u8::is_ascii_digit) {
            i = run(i + 1 + sign, u8::is_ascii_digit);
        }
    }
    i
}

#[cfg(test)]
mod tests;
