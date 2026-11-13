use super::*;
use crate::connection::DbKind::{MySql, Postgres, Sqlite};

fn lex(kind: DbKind, text: &str) -> Vec<(&str, Token)> {
    let mut highlighter = Highlighter::new(kind);
    let mut found = Vec::new();
    for line in text.split('\n') {
        let mut start = 0;
        for (end, token) in highlighter.line(line) {
            if token != Token::Plain {
                found.push((&line[start..end], token));
            }
            start = end;
        }
    }
    found
}

fn only(kind: DbKind, text: &str, token: Token) -> Vec<&str> {
    let mut found = lex(kind, text);
    found.retain(|found| found.1 == token);
    found.into_iter().map(|found| found.0).collect()
}

#[test]
fn words_are_sorted_for_binary_search() {
    assert!(BODY_KEYWORDS.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(words::WORDS.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

#[test]
fn keywords_match_whole_words_in_any_case() {
    assert_eq!(
        lex(Postgres, "select Id FROM users u Where u.order = t.Select"),
        [
            ("select", Token::Keyword),
            ("FROM", Token::Keyword),
            ("Where", Token::Keyword),
        ]
    );
    assert_eq!(lex(Postgres, "selection from_ _from in$x"), []);
}

#[test]
fn numbers_cover_decimals_exponents_and_radixes() {
    let numbers: Vec<&str> = lex(Postgres, "1 2.5 .5 1e-3 0xFF 1_000 t1 a.1 1..3 7-e")
        .into_iter()
        .map(|(text, token)| {
            assert_eq!(token, Token::Number);
            text
        })
        .collect();
    assert_eq!(
        numbers,
        [
            "1", "2.5", ".5", "1e-3", "0xFF", "1_000", "1", "1", "3", "7"
        ]
    );
}

#[test]
fn strings_double_their_quotes_and_span_lines() {
    assert_eq!(lex(Sqlite, "'it''s' x"), [("'it''s'", Token::String)]);
    assert_eq!(
        lex(Postgres, "'a\nfrom\nb' 1"),
        [
            ("'a", Token::String),
            ("from", Token::String),
            ("b'", Token::String),
            ("1", Token::Number),
        ]
    );
}

#[test]
fn backslashes_escape_only_where_the_dialect_says_so() {
    assert_eq!(
        lex(Postgres, r"'a\' 1"),
        [(r"'a\'", Token::String), ("1", Token::Number)]
    );
    assert_eq!(lex(MySql, r"'a\' 1"), [(r"'a\' 1", Token::String)]);
    assert_eq!(
        lex(Postgres, r"E'a\' 1' 2"),
        [(r"E'a\' 1'", Token::String), ("2", Token::Number)]
    );
    assert_eq!(lex(MySql, "'a\\"), [("'a\\", Token::String)]);
}

#[test]
fn quoted_names_follow_the_dialect() {
    assert_eq!(
        lex(Postgres, r#""select" 'x'"#),
        [(r#""select""#, Token::Identifier), ("'x'", Token::String)]
    );
    assert_eq!(lex(MySql, r#""a""b""#), [(r#""a""b""#, Token::String)]);
    assert_eq!(lex(MySql, "`from` 1")[0], ("`from`", Token::Identifier));
    assert_eq!(lex(Sqlite, "`a``b`"), [("`a``b`", Token::Identifier)]);
    assert_eq!(lex(Postgres, "`from`"), [("from", Token::Keyword)]);
    assert_eq!(lex(Sqlite, "[from] 1")[0], ("[from]", Token::Identifier));
    assert_eq!(lex(Postgres, "a[1]"), [("1", Token::Number)]);
}

#[test]
fn comments_run_to_the_line_end_or_the_closing_mark() {
    assert_eq!(
        lex(Postgres, "1 -- select 'x'\n2"),
        [
            ("1", Token::Number),
            ("-- select 'x'", Token::Comment),
            ("2", Token::Number),
        ]
    );
    assert_eq!(
        lex(Sqlite, "a /* b\n\nc */ 2 /*/ 3 */ 4"),
        [
            ("/* b", Token::Comment),
            ("c */", Token::Comment),
            ("2", Token::Number),
            ("/*/ 3 */", Token::Comment),
            ("4", Token::Number),
        ]
    );
}

#[test]
fn comment_rules_differ_between_dialects() {
    let nested = "/* a /* b */ c */ 1";
    assert_eq!(
        lex(Postgres, nested),
        [("/* a /* b */ c */", Token::Comment), ("1", Token::Number)]
    );
    assert_eq!(
        lex(MySql, nested),
        [("/* a /* b */", Token::Comment), ("1", Token::Number)]
    );

    assert_eq!(
        lex(MySql, "1 # x"),
        [("1", Token::Number), ("# x", Token::Comment)]
    );
    assert_eq!(
        lex(Postgres, "1 # 2"),
        [("1", Token::Number), ("2", Token::Number)]
    );

    assert_eq!(
        lex(MySql, "5--2"),
        [("5", Token::Number), ("2", Token::Number)]
    );
    assert_eq!(lex(MySql, "5 -- 2")[1], ("-- 2", Token::Comment));
    assert_eq!(lex(MySql, "5 --")[1], ("--", Token::Comment));
    assert_eq!(lex(Postgres, "5--2")[1], ("--2", Token::Comment));
}

#[test]
fn dollar_quotes_are_postgres_strings() {
    assert_eq!(
        lex(Postgres, "select $body$ select 'x'\n$$ $body$ select"),
        [
            ("select", Token::Keyword),
            ("$body$ select 'x'", Token::String),
            ("$$ $body$", Token::String),
            ("select", Token::Keyword),
        ]
    );
    assert_eq!(
        lex(Postgres, "$$a$$ 1 $$"),
        [
            ("$$a$$", Token::String),
            ("1", Token::Number),
            ("$$", Token::String),
        ]
    );
    assert_eq!(lex(Postgres, "$1 + a$b$ 2"), [("2", Token::Number)]);
    assert_eq!(lex(MySql, "$$ 'a' $$"), [("'a'", Token::String)]);
}

#[test]
fn function_bodies_are_highlighted_as_code() {
    assert_eq!(
        lex(Postgres, "do $$ begin raise notice 'x'; end $$ select 1"),
        [
            ("do", Token::Keyword),
            ("$$", Token::String),
            ("begin", Token::Keyword),
            ("raise", Token::Keyword),
            ("'x'", Token::String),
            ("end", Token::Keyword),
            ("$$", Token::String),
            ("select", Token::Keyword),
            ("1", Token::Number),
        ]
    );
    let function = "create function f() returns int as -- body\n\
                    $body$\n\
                    begin\n  \
                      perform g(1); /* note\n  \
                      $$ */ return 2;\n\
                    end\n\
                    $body$ language plpgsql";
    assert_eq!(
        lex(Postgres, function),
        [
            ("create", Token::Keyword),
            ("function", Token::Keyword),
            ("f", Token::Function),
            ("returns", Token::Keyword),
            ("int", Token::Type),
            ("as", Token::Keyword),
            ("-- body", Token::Comment),
            ("$body$", Token::String),
            ("begin", Token::Keyword),
            ("perform", Token::Keyword),
            ("g", Token::Function),
            ("1", Token::Number),
            ("/* note", Token::Comment),
            ("  $$ */", Token::Comment),
            ("return", Token::Keyword),
            ("2", Token::Number),
            ("end", Token::Keyword),
            ("$body$", Token::String),
            ("language", Token::Keyword),
        ]
    );
    assert_eq!(
        lex(Postgres, "do language plpgsql $$ exit $$ exit")[2..],
        [
            ("$$", Token::String),
            ("exit", Token::Keyword),
            ("$$", Token::String),
        ]
    );
}

#[test]
fn only_bodies_after_as_or_do_become_code() {
    assert_eq!(
        lex(Postgres, "select $$it's from$$ 1"),
        [
            ("select", Token::Keyword),
            ("$$it's from$$", Token::String),
            ("1", Token::Number),
        ]
    );
    assert_eq!(
        lex(Postgres, "x.as $$ from $$"),
        [("$$ from $$", Token::String)]
    );
    assert_eq!(
        lex(Postgres, "do $$ execute $q$ select $q$ $$"),
        [
            ("do", Token::Keyword),
            ("$$", Token::String),
            ("execute", Token::Keyword),
            ("$q$ select $q$", Token::String),
            ("$$", Token::String),
        ]
    );
    assert_eq!(
        lex(MySql, "do $$ 1"),
        [("do", Token::Keyword), ("1", Token::Number)]
    );
}

#[test]
fn a_body_always_ends_at_its_closing_delimiter() {
    assert_eq!(
        lex(Postgres, "do $$ select 'oops\n/* $$ select 1"),
        [
            ("do", Token::Keyword),
            ("$$", Token::String),
            ("select", Token::Keyword),
            ("'oops", Token::String),
            ("/* $$", Token::String),
            ("select", Token::Keyword),
            ("1", Token::Number),
        ]
    );
    assert_eq!(
        lex(Postgres, "do $a$ 1 as $b$ 2 $a$ 3 $b$ 4"),
        [
            ("do", Token::Keyword),
            ("$a$", Token::String),
            ("1", Token::Number),
            ("as", Token::Keyword),
            ("$b$", Token::String),
            ("2", Token::Number),
            ("$a$", Token::String),
            ("3", Token::Number),
            ("$b$ 4", Token::String),
        ]
    );
}

#[test]
fn parameters_stay_plain_and_casts_keep_their_type() {
    assert_eq!(lex(Postgres, ":select $2 x::int"), [("int", Token::Type)]);
    assert_eq!(lex(MySql, "@from := 1"), [("1", Token::Number)]);
    assert_eq!(lex(Sqlite, "$from @in :as"), []);
}

#[test]
fn names_that_look_like_keywords_stay_plain() {
    let columns = "select date, text, key, first from events where time is null order by json desc";
    assert_eq!(
        only(Postgres, columns, Token::Keyword),
        [
            "select", "from", "where", "is", "null", "order", "by", "desc"
        ]
    );
    assert_eq!(only(Postgres, columns, Token::Type), [""; 0]);
    assert_eq!(
        lex(Sqlite, "select\n  key,\n  comment\nfrom notes"),
        [("select", Token::Keyword), ("from", Token::Keyword)]
    );
    assert_eq!(
        only(
            Postgres,
            "select n.type as role, d as last from n",
            Token::Keyword
        ),
        ["select", "as", "as", "from"]
    );
    assert_eq!(
        only(
            Postgres,
            "select * from a join b on type = 1",
            Token::Keyword
        ),
        ["select", "from", "join", "on"]
    );
}

#[test]
fn soft_keywords_count_where_a_keyword_can_stand() {
    assert_eq!(
        lex(Postgres, "create table t (key int primary key, first text)"),
        [
            ("create", Token::Keyword),
            ("table", Token::Keyword),
            ("int", Token::Type),
            ("primary", Token::Keyword),
            ("key", Token::Keyword),
            ("text", Token::Type),
        ]
    );
    assert_eq!(
        only(
            Postgres,
            "select a from t order by a desc nulls first fetch first 5 rows only",
            Token::Keyword
        ),
        [
            "select", "from", "order", "by", "desc", "nulls", "first", "fetch", "first", "rows",
            "only"
        ]
    );
    assert_eq!(
        only(
            Postgres,
            "begin; comment on column t.c is 'x'; commit",
            Token::Keyword
        ),
        ["begin", "comment", "on", "column", "is", "commit"]
    );
    assert_eq!(lex(MySql, "proc: begin"), [("begin", Token::Keyword)]);
}

#[test]
fn each_dialect_reserves_its_own_words() {
    let reserved = |kind| only(kind, "select key, user from t", Token::Keyword);
    assert_eq!(reserved(Postgres), ["select", "user", "from"]);
    assert_eq!(reserved(MySql), ["select", "key", "from"]);
    assert_eq!(reserved(Sqlite), ["select", "from"]);

    let foreign = |kind| only(kind, "pragma a = b regexp c ilike d", Token::Keyword);
    assert_eq!(foreign(Postgres), ["ilike"]);
    assert_eq!(foreign(MySql), ["regexp"]);
    assert_eq!(foreign(Sqlite), ["pragma", "regexp"]);
}

#[test]
fn neighbours_turn_soft_keywords_on_inside_expressions() {
    let window = "sum(x) over (partition by a rows between unbounded preceding and current row)";
    assert_eq!(
        only(Postgres, window, Token::Keyword),
        [
            "over",
            "partition",
            "by",
            "rows",
            "between",
            "unbounded",
            "preceding",
            "and",
            "current",
            "row"
        ]
    );
    assert_eq!(
        only(
            Postgres,
            "select partition, current, rows from t",
            Token::Keyword
        ),
        ["select", "from"]
    );
    assert_eq!(
        only(
            Postgres,
            "insert into t (id) values (1) on conflict (id) do nothing",
            Token::Keyword
        ),
        [
            "insert", "into", "values", "on", "conflict", "do", "nothing"
        ]
    );
    assert_eq!(
        only(
            Sqlite,
            "insert or replace into t values (1)",
            Token::Keyword
        ),
        ["insert", "or", "replace", "into", "values"]
    );
    assert_eq!(
        only(
            MySql,
            "create table if not exists t (a int)",
            Token::Keyword
        ),
        ["create", "table", "if", "not", "exists"]
    );
    assert_eq!(
        only(
            Sqlite,
            "select a from t where a not like 'x'",
            Token::Keyword
        ),
        ["select", "from", "where", "not", "like"]
    );
    assert_eq!(
        only(
            Postgres,
            "select a from t group by rollup (a), cube(b)",
            Token::Keyword
        ),
        ["select", "from", "group", "by", "rollup", "cube"]
    );
}

#[test]
fn the_statement_verb_decides_what_follows_set_and_on() {
    assert_eq!(
        only(Postgres, "update t set role = 'x'", Token::Keyword),
        ["update", "set"]
    );
    assert_eq!(
        only(Postgres, "set role admin", Token::Keyword),
        ["set", "role"]
    );
    assert_eq!(
        only(
            Postgres,
            "grant select on schema public to app",
            Token::Keyword
        ),
        ["grant", "select", "on", "schema", "to"]
    );
    assert_eq!(
        lex(
            Postgres,
            "alter table t alter column c type text, drop column d"
        ),
        [
            ("alter", Token::Keyword),
            ("table", Token::Keyword),
            ("alter", Token::Keyword),
            ("column", Token::Keyword),
            ("type", Token::Keyword),
            ("text", Token::Type),
            ("drop", Token::Keyword),
            ("column", Token::Keyword),
        ]
    );
}

#[test]
fn type_names_are_types_only_where_a_type_can_stand() {
    assert_eq!(
        lex(
            Postgres,
            "create table n (date date, text text not null, at timestamp with time zone)"
        ),
        [
            ("create", Token::Keyword),
            ("table", Token::Keyword),
            ("date", Token::Type),
            ("text", Token::Type),
            ("not", Token::Keyword),
            ("null", Token::Keyword),
            ("timestamp", Token::Type),
            ("with", Token::Keyword),
            ("time", Token::Keyword),
            ("zone", Token::Keyword),
        ]
    );
    assert_eq!(
        lex(
            Postgres,
            "select x::my_enum, cast(y as date), date '2024-01-01', z as date from t"
        ),
        [
            ("select", Token::Keyword),
            ("my_enum", Token::Type),
            ("cast", Token::Keyword),
            ("as", Token::Keyword),
            ("date", Token::Type),
            ("date", Token::Type),
            ("'2024-01-01'", Token::String),
            ("as", Token::Keyword),
            ("from", Token::Keyword),
        ]
    );
    assert_eq!(
        only(
            Postgres,
            "create function f(text, int) returns record",
            Token::Type
        ),
        ["text", "int", "record"]
    );
    assert_eq!(
        only(MySql, "select cast(a as unsigned), b from t", Token::Type),
        ["unsigned"]
    );
    assert_eq!(
        only(
            Sqlite,
            "create table t (a integer, text text, b)",
            Token::Type
        ),
        ["integer", "text"]
    );
}

#[test]
fn a_word_before_a_parenthesis_in_an_expression_is_a_function() {
    assert_eq!(
        lex(
            Postgres,
            "select count(*), left(name, 1), pg_catalog.lower(x) from a left join b using (id)"
        ),
        [
            ("select", Token::Keyword),
            ("count", Token::Function),
            ("left", Token::Function),
            ("1", Token::Number),
            ("lower", Token::Function),
            ("from", Token::Keyword),
            ("left", Token::Keyword),
            ("join", Token::Keyword),
            ("using", Token::Keyword),
        ]
    );
    assert_eq!(
        only(
            Postgres,
            "select time, date(created), date from t",
            Token::Function
        ),
        ["date"]
    );
    assert_eq!(
        only(
            MySql,
            "select if(a, b, c), values(d), char(e) from t",
            Token::Function
        ),
        ["if", "values", "char"]
    );
    let names = "insert into t(a) values(1); create index i on t(a); \
                 alter table o add foreign key (u) references users(id)";
    assert_eq!(only(Postgres, names, Token::Function), [""; 0]);
    assert_eq!(
        only(
            Postgres,
            "create view v as select a from b join c on lower(c.x) = b.y",
            Token::Function
        ),
        ["lower"]
    );
}

#[test]
fn statements_inside_a_body_start_after_then_and_else() {
    assert_eq!(
        lex(
            Postgres,
            "do $$ begin if key > 0 then return new; else raise notice 'x'; end if; end $$"
        ),
        [
            ("do", Token::Keyword),
            ("$$", Token::String),
            ("begin", Token::Keyword),
            ("if", Token::Keyword),
            ("0", Token::Number),
            ("then", Token::Keyword),
            ("return", Token::Keyword),
            ("else", Token::Keyword),
            ("raise", Token::Keyword),
            ("'x'", Token::String),
            ("end", Token::Keyword),
            ("if", Token::Keyword),
            ("end", Token::Keyword),
            ("$$", Token::String),
        ]
    );
    assert_eq!(
        only(
            Postgres,
            "do $$ begin for r in execute 'x' loop end loop; end $$",
            Token::Keyword
        ),
        [
            "do", "begin", "for", "in", "execute", "loop", "end", "loop", "end"
        ]
    );
}

#[test]
fn multibyte_text_keeps_token_boundaries() {
    assert_eq!(
        lex(Postgres, "select 'привет' as имя, \"名前\" -- коммент"),
        [
            ("select", Token::Keyword),
            ("'привет'", Token::String),
            ("as", Token::Keyword),
            ("\"名前\"", Token::Identifier),
            ("-- коммент", Token::Comment),
        ]
    );
}

#[test]
fn every_prefix_of_tricky_input_is_fully_covered() {
    let samples = [
        "select 'ы\\' 1e+ 0x .5. $тег$ я $тег$ /* /* */ */ -- ё",
        "E'\\é' e'x' $$ $a$b$ $1 :: :x @ё `ы` [я] \"ю\"\"\" #é",
        "1.e5 1e 0b 0xZ 9..9 .. .9.9 a.b.c - -- --- / /* /*/ */",
        "'\\",
        "$",
        "do $я$ 'ы as $b$ /* é $я$ 'ю' as $$ é $b$ $$ do $",
    ];
    for kind in [Postgres, MySql, Sqlite] {
        for sample in samples {
            let ends = sample
                .char_indices()
                .map(|(at, _)| at)
                .chain([sample.len()]);
            for end in ends {
                let prefix = &sample[..end];
                let tokens = Highlighter::new(kind).line(prefix);
                let mut start = 0;
                for &(end, _) in &tokens {
                    assert!(end > start, "{kind:?} {prefix:?}");
                    assert!(prefix.is_char_boundary(end), "{kind:?} {prefix:?}");
                    start = end;
                }
                assert_eq!(start, prefix.len(), "{kind:?} {prefix:?}");
            }
        }
    }
}
