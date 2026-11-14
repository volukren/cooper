use std::time::Instant;

use super::*;
use crate::connection::PasswordStorage;

const USERS: &str = "Users";
const EMPTY: &str = "empty";
const WEIRD: &str = "we\"ird ta`ble ü";
const WEIRD_COLUMN: &str = "sel\"ect `col";
const VIEW: &str = "users_view";

fn connection(kind: DbKind, port: Option<u16>, user: &str, database: &str) -> Connection {
    Connection {
        name: "test".to_string(),
        kind,
        tag: None,
        read_only: false,
        host: if kind.is_file_based() {
            String::new()
        } else {
            "127.0.0.1".to_string()
        },
        port,
        user: user.to_string(),
        database: database.to_string(),
        password_storage: PasswordStorage::Keychain,
        password: String::new(),
    }
}

fn sqlite_connection(path: &std::path::Path) -> Connection {
    connection(DbKind::Sqlite, None, "", path.to_str().unwrap())
}

fn wait(session: &Session) -> Response {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(response) = session.try_recv() {
            return response;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the worker"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn wait_for(session: &Session, id: u64) -> Response {
    loop {
        let response = wait(session);
        if response.id() == Some(id) {
            return response;
        }
    }
}

fn page(session: &Session, schema: &str, table: &str, offset: u64, limit: u32) -> Page {
    let id = session.fetch_page(schema, table, offset, limit);
    match wait_for(session, id) {
        Response::Page { page, .. } => page,
        other => panic!("expected a page, got {other:?}"),
    }
}

fn count(session: &Session, schema: &str, table: &str) -> u64 {
    let id = session.count_rows(schema, table);
    match wait_for(session, id) {
        Response::Count { count, .. } => count,
        other => panic!("expected a count, got {other:?}"),
    }
}

fn script(session: &Session, sql: &str, limit: u32) -> Result<ScriptResult, String> {
    let id = session.run_script(sql, limit);
    match wait_for(session, id) {
        Response::Script { result, .. } => Ok(result),
        Response::Error { message, .. } => Err(message),
        other => panic!("expected a script result, got {other:?}"),
    }
}

fn ddl(session: &Session, schema: &str, table: &str) -> Result<TableDdl, String> {
    let id = session.fetch_ddl(schema, table);
    match wait_for(session, id) {
        Response::Ddl { ddl, .. } => Ok(ddl),
        Response::Error { message, .. } => Err(message),
        other => panic!("expected a definition, got {other:?}"),
    }
}

fn definitions(ddl: &TableDdl) -> Vec<(&str, &str)> {
    ddl.columns
        .iter()
        .map(|column| (column.name.as_str(), column.definition.as_str()))
        .collect()
}

fn cells(rows: &[&[Option<&str>]]) -> Vec<Vec<Option<String>>> {
    rows.iter()
        .map(|row| row.iter().map(|cell| cell.map(str::to_string)).collect())
        .collect()
}

fn exercise_scripts(session: &Session, kind: DbKind, schema: &str) {
    let notes = qualified(kind, schema, "script notes").unwrap();
    let users = qualified(kind, schema, USERS).unwrap();
    let text = match kind {
        DbKind::MySql => "VARCHAR(20)",
        _ => "TEXT",
    };
    let result = script(
        session,
        &format!(
            "CREATE TABLE {notes} (id INTEGER PRIMARY KEY, body {text});\n\
             INSERT INTO {notes} VALUES (1, 'a;b'), (2, NULL), (3, 'c'); -- three rows\n\
             UPDATE {notes} SET body = 'ü' WHERE id = 2;\n\
             SELECT 'ignored';\n\
             SELECT id, body FROM {notes} WHERE id < 3 ORDER BY id"
        ),
        100,
    )
    .unwrap();
    assert_eq!(result.statements, 5);
    assert_eq!(result.result_sets, 2);
    assert_eq!(result.affected, 4);
    assert!(!result.truncated);
    let names: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "body"]);
    assert_eq!(
        result.rows,
        cells(&[&[Some("1"), Some("a;b")], &[Some("2"), Some("ü")]])
    );
    assert_eq!(count(session, schema, "script notes"), 3);

    let all = format!("SELECT id FROM {users} ORDER BY id");
    let limited = script(session, &all, 10).unwrap();
    assert_eq!(limited.rows.len(), 10);
    assert!(limited.truncated);
    assert_eq!(limited.rows[9], [Some("10".to_string())]);
    assert_eq!((limited.statements, limited.affected), (1, 0));
    let exact = script(session, &all, 250).unwrap();
    assert_eq!(exact.rows.len(), 250);
    assert!(!exact.truncated);
    let none = script(session, &all, 0).unwrap();
    assert!(none.rows.is_empty());
    assert!(none.truncated);
    assert_eq!(none.columns.len(), 1);

    let product = format!("SELECT a.id FROM {users} a, {users} b");
    let streamed = script(session, &product, 100).unwrap();
    assert_eq!(streamed.rows.len(), 100);
    assert!(streamed.truncated);

    let empty = script(session, &format!("SELECT id FROM {users} WHERE id < 0"), 10).unwrap();
    assert_eq!((empty.result_sets, empty.columns.len()), (1, 1));
    assert!(empty.rows.is_empty());
    assert!(!empty.truncated);

    let failed = script(
        session,
        &format!(
            "DELETE FROM {notes} WHERE id = 3; SELECT * FROM {notes} WHERE no_such_column = 1"
        ),
        10,
    );
    assert!(failed.unwrap_err().contains("no_such_column"));
    assert!(script(session, "SELEC 1", 10).is_err());
    let dropped = script(session, &format!("DROP TABLE {notes}"), 10).unwrap();
    assert_eq!((dropped.statements, dropped.result_sets), (1, 0));
    assert!(dropped.columns.is_empty());
    assert!(script(session, &format!("SELECT * FROM {notes}"), 10).is_err());
}

fn seed_statements(kind: DbKind, schema: &str) -> Vec<String> {
    let q = |name: &str| quote_ident(kind, name);
    let t = |name: &str| qualified(kind, schema, name).unwrap();
    let (text, blob, bytes) = match kind {
        DbKind::Postgres => ("TEXT", "BYTEA", "'\\xdeadbeef'::bytea"),
        DbKind::MySql => ("VARCHAR(2000)", "BLOB", "X'deadbeef'"),
        DbKind::Sqlite => ("TEXT", "BLOB", "X'deadbeef'"),
    };
    let long = "x".repeat(600);
    let newline = match kind {
        DbKind::Postgres => "' || chr(10) || chr(9) || '",
        DbKind::MySql => "\\n\\t",
        DbKind::Sqlite => "' || char(10) || char(9) || '",
    };
    let mut statements = vec![
        format!(
            "CREATE TABLE {} (id INTEGER PRIMARY KEY, name {text} NOT NULL, bio {text}, data {blob})",
            t(USERS)
        ),
        format!("CREATE TABLE {} (a INTEGER, b {text})", t(EMPTY)),
        format!("CREATE TABLE {} ({} {text})", t(WEIRD), q(WEIRD_COLUMN)),
        format!("INSERT INTO {} VALUES ('it''s; DROP TABLE x --')", t(WEIRD)),
    ];
    for id in (1..=250).rev() {
        let bio = if id % 2 == 0 {
            "NULL".to_string()
        } else {
            format!("'line{newline}{long}'")
        };
        statements.push(format!(
            "INSERT INTO {} VALUES ({id}, 'user {id}', {bio}, {bytes})",
            t(USERS)
        ));
    }
    let view = match kind {
        DbKind::Sqlite => q(VIEW),
        _ => t(VIEW),
    };
    statements.push(format!(
        "CREATE VIEW {view} AS SELECT id, name FROM {}",
        t(USERS)
    ));
    statements
}

fn exercise(session: &Session, kind: DbKind, schema: &str) {
    match wait(session) {
        Response::Connected { server_info } => assert!(!server_info.is_empty()),
        other => panic!("expected Connected, got {other:?}"),
    }

    let id = session.list_schemas();
    match wait_for(session, id) {
        Response::Schemas { schemas, .. } => {
            assert!(schemas.iter().any(|s| s == schema), "{schemas:?}")
        }
        other => panic!("expected schemas, got {other:?}"),
    }

    let id = session.list_tables(schema);
    match wait_for(session, id) {
        Response::Tables {
            schema: echoed,
            tables,
            ..
        } => {
            assert_eq!(echoed, schema);
            let kind = |name: &str| tables.iter().find(|t| t.name == name).map(|t| t.kind);
            assert_eq!(kind(USERS), Some(TableKind::Table), "{tables:?}");
            assert_eq!(kind(EMPTY), Some(TableKind::Table));
            assert_eq!(kind(WEIRD), Some(TableKind::Table));
            assert_eq!(kind(VIEW), Some(TableKind::View));
            assert!(tables.iter().all(|t| !t.name.starts_with("sqlite_")));
        }
        other => panic!("expected tables, got {other:?}"),
    }

    let first = page(session, schema, USERS, 0, 100);
    assert_eq!(
        (first.schema.as_str(), first.table.as_str()),
        (schema, USERS)
    );
    let names: Vec<&str> = first.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "name", "bio", "data"]);
    assert!(first.columns.iter().all(|c| !c.type_name.is_empty()));
    assert_eq!(first.rows.len(), 100);
    assert!(first.has_more);
    assert_eq!(first.offset, 0);
    assert_eq!(first.rows[0][0].as_deref(), Some("1"));
    assert_eq!(first.rows[99][0].as_deref(), Some("100"));
    assert_eq!(first.rows[0][1].as_deref(), Some("user 1"));
    let bio = first.rows[0][2].as_deref().unwrap();
    assert!(bio.starts_with("line↵→xxx"), "{bio}");
    assert!(bio.ends_with('…'));
    assert_eq!(bio.chars().count(), MAX_CELL_CHARS + 1);
    let full = &first.full[&(0, 2)];
    assert_eq!(full.text, format!("line\n\t{}", "x".repeat(600)));
    assert!(!full.truncated);
    assert_eq!(first.full.len(), 50, "only the rewritten cells are kept");
    assert_eq!(first.rows[1][2], None);
    assert_eq!(first.rows[0][3].as_deref(), Some("0xdeadbeef"));

    let last = page(session, schema, USERS, 200, 100);
    assert_eq!(last.rows.len(), 50);
    assert!(!last.has_more);
    assert_eq!(last.rows[49][0].as_deref(), Some("250"));

    let exact = page(session, schema, USERS, 150, 100);
    assert_eq!(exact.rows.len(), 100);
    assert!(!exact.has_more);

    let beyond = page(session, schema, USERS, 300, 100);
    assert!(beyond.rows.is_empty());
    assert_eq!(beyond.columns.len(), 4);
    assert!(!beyond.has_more);

    assert_eq!(count(session, schema, USERS), 250);

    let users = ddl(session, schema, USERS).unwrap();
    assert!(users.sql.starts_with("CREATE TABLE "), "{}", users.sql);
    assert!(users.sql.ends_with(';'), "{}", users.sql);
    assert!(users.sql.contains("bio"), "{}", users.sql);
    let names: Vec<&str> = users.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "name", "bio", "data"]);
    let id = users.columns[0].definition.to_uppercase();
    assert!(id.contains("INT") && id.ends_with("PRIMARY KEY"), "{id}");
    let name = users.columns[1].definition.to_uppercase();
    assert!(name.ends_with(" NOT NULL"), "{name}");
    assert!(!users.columns[2].definition.contains("NOT NULL"));
    let view = ddl(session, schema, VIEW).unwrap();
    assert!(view.sql.contains("VIEW"), "{}", view.sql);
    assert_eq!(view.columns.len(), 2);
    let weird = ddl(session, schema, WEIRD).unwrap();
    assert_eq!(weird.columns[0].name, WEIRD_COLUMN);
    let missing = ddl(session, schema, "no such table").unwrap_err();
    assert!(!missing.is_empty());

    let empty = page(session, schema, EMPTY, 0, 100);
    let names: Vec<&str> = empty.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
    assert!(empty.rows.is_empty());
    assert!(!empty.has_more);
    assert_eq!(count(session, schema, EMPTY), 0);

    let weird = page(session, schema, WEIRD, 0, 100);
    assert_eq!(weird.columns[0].name, WEIRD_COLUMN);
    assert_eq!(weird.rows, [[Some("it's; DROP TABLE x --".to_string())]]);
    assert_eq!(count(session, schema, WEIRD), 1);

    let view = page(session, schema, VIEW, 0, 100);
    assert_eq!(view.columns.len(), 2);
    assert_eq!(view.rows.len(), 100);
    assert!(view.has_more);
    assert_eq!(count(session, schema, VIEW), 250);

    for (missing_schema, missing_table) in [
        (schema, "no such table"),
        (schema, "x\"; DROP TABLE \"Users"),
        ("no such schema", USERS),
        (schema, ""),
        (schema, "nul\0name"),
    ] {
        let id = session.fetch_page(missing_schema, missing_table, 0, 100);
        match wait_for(session, id) {
            Response::Error { message, .. } => assert!(!message.is_empty()),
            other => panic!("expected an error, got {other:?}"),
        }
    }
    assert_eq!(count(session, schema, USERS), 250);

    let mut newest = 0;
    for offset in [0, 100, 200, 100, 0, 200] {
        newest = session.fetch_page(schema, USERS, offset, 100);
    }
    match wait_for(session, newest) {
        Response::Page { page, .. } => assert_eq!(page.offset, 200),
        other => panic!("expected a page, got {other:?}"),
    }

    exercise_scripts(session, kind, schema);
}

fn seeded_sqlite() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test db.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(&seed_statements(DbKind::Sqlite, "main").join(";\n"))
        .unwrap();
    db.execute_batch(
        "CREATE TABLE no_pk (v TEXT);
         INSERT INTO no_pk VALUES ('b'), ('a'), ('c');
         CREATE TABLE pair (a INTEGER, b INTEGER, v TEXT, PRIMARY KEY (b, a)) WITHOUT ROWID;
         INSERT INTO pair VALUES (2, 1, 'second'), (1, 2, 'third'), (1, 1, 'first');
         CREATE TABLE kinds (i INTEGER, r REAL, t TEXT, b BLOB, n);
         INSERT INTO kinds VALUES (-7, 1.5, 'ю😀', zeroblob(40), NULL), (0, 0.0, '', X'', NULL);
         CREATE TABLE indexed (v TEXT UNIQUE);
         CREATE VIEW slow AS
           WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 100000000000)
           SELECT x FROM c;",
    )
    .unwrap();
    (dir, path)
}

#[test]
fn quotes_identifiers_per_dialect() {
    assert_eq!(quote_ident(DbKind::Postgres, "users"), "\"users\"");
    assert_eq!(quote_ident(DbKind::Postgres, "a\"b"), "\"a\"\"b\"");
    assert_eq!(quote_ident(DbKind::Sqlite, "a\"\"b`c"), "\"a\"\"\"\"b`c\"");
    assert_eq!(quote_ident(DbKind::MySql, "users"), "`users`");
    assert_eq!(quote_ident(DbKind::MySql, "a`b\"c"), "`a``b\"c`");
    assert_eq!(quote_ident(DbKind::MySql, "ünï Côde"), "`ünï Côde`");
    assert_eq!(quote_ident(DbKind::Postgres, ""), "\"\"");
}

#[test]
fn builds_safe_statements() {
    assert_eq!(
        qualified(DbKind::Postgres, "My Schema", "t\"; DROP TABLE x; --").unwrap(),
        "\"My Schema\".\"t\"\"; DROP TABLE x; --\""
    );
    assert_eq!(
        qualified(DbKind::MySql, "db", "t`; DROP").unwrap(),
        "`db`.`t``; DROP`"
    );
    assert!(qualified(DbKind::Postgres, "public", "a\0b").is_err());
    assert!(qualified(DbKind::Postgres, "", "a").is_err());
    assert!(qualified(DbKind::Sqlite, "main", "").is_err());

    assert_eq!(
        select_sql(DbKind::Sqlite, "main", "t", &[], 0, 100).unwrap(),
        "SELECT * FROM \"main\".\"t\" LIMIT 101 OFFSET 0"
    );
    let pk = ["b`x".to_string(), "a".to_string()];
    assert_eq!(
        select_sql(DbKind::MySql, "db", "t", &pk, 300, 100).unwrap(),
        "SELECT * FROM `db`.`t` ORDER BY `b``x`, `a` LIMIT 101 OFFSET 300"
    );
    assert_eq!(
        select_sql(DbKind::Postgres, "s", "t", &[], u64::MAX, u32::MAX).unwrap(),
        format!(
            "SELECT * FROM \"s\".\"t\" LIMIT {} OFFSET {}",
            u64::from(u32::MAX) + 1,
            u64::MAX
        )
    );
    assert_eq!(
        count_sql(DbKind::Postgres, "s", "t").unwrap(),
        "SELECT COUNT(*) FROM \"s\".\"t\""
    );
}

fn row_update(table: &str, key: &[(&str, &str)], values: &[(&str, Option<&str>)]) -> RowUpdate {
    RowUpdate {
        schema: "main".to_string(),
        table: table.to_string(),
        key: key
            .iter()
            .map(|(c, v)| (c.to_string(), v.to_string()))
            .collect(),
        values: values
            .iter()
            .map(|(c, v)| (c.to_string(), v.map(str::to_string)))
            .collect(),
    }
}

fn update(session: &Session, update: RowUpdate) -> Result<(), String> {
    let id = session.update_row(update);
    match wait_for(session, id) {
        Response::RowUpdated { .. } => Ok(()),
        Response::Error { message, .. } => Err(message),
        other => panic!("expected an update result, got {other:?}"),
    }
}

#[test]
fn builds_update_statements() {
    let edit = RowUpdate {
        schema: "my db".to_string(),
        table: r#"t`"x"#.to_string(),
        key: vec![
            ("b`k".to_string(), "1".to_string()),
            ("a".to_string(), "x'y".to_string()),
        ],
        values: vec![
            ("name".to_string(), Some("it's".to_string())),
            ("note".to_string(), None),
        ],
    };
    assert_eq!(
        update_sql(DbKind::MySql, &edit, placeholder).unwrap(),
        r#"UPDATE `my db`.`t``"x` SET `name` = ?, `note` = NULL WHERE `b``k` = ? AND `a` = ?"#
    );
    assert_eq!(update_params(&edit), ["it's", "1", "x'y"]);
    assert_eq!(
        update_sql(DbKind::Sqlite, &edit, placeholder).unwrap(),
        r#"UPDATE "my db"."t`""x" SET "name" = ?, "note" = NULL WHERE "b`k" = ? AND "a" = ?"#
    );
    assert_eq!(
        update_sql(DbKind::Postgres, &edit, |v| v
            .map_or_else(|| "NULL".into(), pg_literal))
        .unwrap(),
        r#"UPDATE "my db"."t`""x" SET "name" = 'it''s', "note" = NULL WHERE "b`k" = '1' AND "a" = 'x''y'"#
    );

    let mut nothing = edit.clone();
    nothing.values.clear();
    assert!(update_sql(DbKind::Postgres, &nothing, placeholder).is_err());
    let mut keyless = edit.clone();
    keyless.key.clear();
    assert!(update_sql(DbKind::Postgres, &keyless, placeholder).is_err());
    let mut nul = edit;
    nul.table = "a\0b".to_string();
    assert!(update_sql(DbKind::Postgres, &nul, placeholder).is_err());
}

#[test]
fn quotes_postgres_literals_whatever_the_backslash_setting() {
    assert_eq!(pg_literal(""), "''");
    assert_eq!(pg_literal("plain"), "'plain'");
    assert_eq!(pg_literal("it's"), "'it''s'");
    assert_eq!(pg_literal(r"a\b'"), r"E'a\\b'''");
    assert_eq!(
        pg_literal(r"\'; DROP TABLE x; --"),
        r"E'\\''; DROP TABLE x; --'"
    );
    assert_eq!(pg_literal("line\nü"), "'line\nü'");
}

#[test]
fn reads_matched_rows_from_mysql_info() {
    assert_eq!(
        my_rows_matched("Rows matched: 1  Changed: 0  Warnings: 0"),
        Some(1)
    );
    assert_eq!(
        my_rows_matched("Rows matched: 12  Changed: 3  Warnings: 1"),
        Some(12)
    );
    assert_eq!(my_rows_matched(""), None);
    assert!(matched_one(1).is_ok());
    assert!(matched_one(0).unwrap_err().message.contains("not found"));
    assert!(
        matched_one(2)
            .unwrap_err()
            .message
            .contains("matches 2 rows")
    );
}

#[test]
fn renders_text_on_one_line() {
    assert_eq!(render_text("plain"), "plain");
    assert_eq!(render_text(""), "");
    assert_eq!(render_text("a\nb\r\nc\rd\te"), "a↵b↵c↵d→e");
    assert_eq!(render_text("a\u{0}b\u{1b}c\u{7f}d\u{2028}e"), "a·b·c·d↵e");
    assert_eq!(render_text("日本語 😀"), "日本語 😀");
}

#[test]
fn truncates_long_text_on_char_boundary() {
    let exact = "é".repeat(MAX_CELL_CHARS);
    assert_eq!(render_text(&exact), exact);
    let long = "😀".repeat(MAX_CELL_CHARS + 50);
    let rendered = render_text(&long);
    assert_eq!(rendered.chars().count(), MAX_CELL_CHARS + 1);
    assert!(rendered.ends_with('…'));
    assert!(rendered.starts_with("😀😀"));
}

#[test]
fn keeps_the_original_of_rewritten_text() {
    assert_eq!(Field::text("plain").full, None);
    assert_eq!(Field::text("").full, None);

    let field = Field::text("a\r\nb\tc");
    assert_eq!(field.shown.as_deref(), Some("a↵b→c"));
    let full = field.full.unwrap();
    assert_eq!(full.text, "a\r\nb\tc");
    assert!(!full.truncated);

    let exact = "x".repeat(MAX_FULL_BYTES);
    let full = Field::text(&exact).full.unwrap();
    assert_eq!(full.text, exact);
    assert!(!full.truncated);

    let long = format!("x{}", "é".repeat(MAX_FULL_BYTES));
    let full = Field::text(&long).full.unwrap();
    assert_eq!(full.text.len(), MAX_FULL_BYTES - 1);
    assert!(long.starts_with(&full.text));
    assert!(full.truncated);
}

#[test]
fn sqlite_script_keeps_full_values_of_the_last_result_set() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    let multiline = "SELECT 1, 'a' || char(10) || 'b' UNION ALL SELECT 2, 'c' || char(10) || 'd'";
    let result = script(&session, multiline, 1).unwrap();
    assert_eq!(result.rows, cells(&[&[Some("1"), Some("a↵b")]]));
    assert_eq!(result.full.len(), 1, "skipped rows leave nothing behind");
    assert_eq!(result.full[&(0, 1)].text, "a\nb");

    let result = script(&session, &format!("{multiline}; SELECT 'plain'"), 10).unwrap();
    assert_eq!(result.rows, cells(&[&[Some("plain")]]));
    assert!(result.full.is_empty());
}

#[test]
fn describes_columns_from_the_catalog_rows() {
    let row = |fields: &[&str]| -> Vec<String> { fields.iter().map(|f| f.to_string()).collect() };

    let my = |fields: &[&str]| my_column_definition(&row(fields)).definition;
    assert_eq!(
        my(&["id", "int", "NO", "0", "", "auto_increment", "PRI", ""]),
        "int NOT NULL AUTO_INCREMENT PRIMARY KEY"
    );
    assert_eq!(
        my(&["s", "varchar(9)", "YES", "1", "it's", "", "UNI", ""]),
        "varchar(9) DEFAULT 'it''s' UNIQUE"
    );
    assert_eq!(
        my(&["s", "varchar(9)", "YES", "1", "'x'", "", "", ""]),
        "varchar(9) DEFAULT 'x'"
    );
    assert_eq!(
        my(&["s", "char(3)", "YES", "1", "007", "", "", ""]),
        "char(3) DEFAULT '007'"
    );
    assert_eq!(
        my(&["n", "decimal(4,2)", "NO", "1", "1.50", "", "", ""]),
        "decimal(4,2) NOT NULL DEFAULT 1.50"
    );
    assert_eq!(
        my(&["n", "int(11)", "YES", "1", "NULL", "", "", ""]),
        "int(11)"
    );
    assert_eq!(
        my(&["f", "bit(3)", "YES", "1", "b'101'", "", "", ""]),
        "bit(3) DEFAULT b'101'"
    );
    assert_eq!(
        my(&[
            "u",
            "char(36)",
            "YES",
            "1",
            "uuid()",
            "DEFAULT_GENERATED",
            "",
            ""
        ]),
        "char(36) DEFAULT uuid()"
    );
    assert_eq!(
        my(&[
            "t",
            "timestamp",
            "NO",
            "1",
            "CURRENT_TIMESTAMP",
            "DEFAULT_GENERATED on update CURRENT_TIMESTAMP",
            "",
            ""
        ]),
        "timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP"
    );
    assert_eq!(
        my(&[
            "g",
            "int",
            "YES",
            "1",
            "NULL",
            "VIRTUAL GENERATED",
            "",
            "`n` * 2"
        ]),
        "int GENERATED ALWAYS AS (`n` * 2) VIRTUAL"
    );

    let pg = |fields: &[&str]| pg_column_definition(&row(fields));
    let (column, line) = pg(&["id", "id", "integer", "true", "", "a", "", "true"]);
    assert_eq!(column.name, "id");
    assert_eq!(
        column.definition,
        "integer NOT NULL GENERATED ALWAYS AS IDENTITY PRIMARY KEY"
    );
    assert_eq!(line, "id integer NOT NULL GENERATED ALWAYS AS IDENTITY");
    let (column, line) = pg(&[
        "My Col",
        "\"My Col\"",
        "text",
        "false",
        "'x'::text",
        "",
        "",
        "false",
    ]);
    assert_eq!(column.name, "My Col");
    assert_eq!(column.definition, "text DEFAULT 'x'::text");
    assert_eq!(line, "\"My Col\" text DEFAULT 'x'::text");
    let (column, _) = pg(&["n", "n", "bigint", "true", "", "d", "", "false"]);
    assert_eq!(
        column.definition,
        "bigint NOT NULL GENERATED BY DEFAULT AS IDENTITY"
    );
    let (column, _) = pg(&["g", "g", "integer", "false", "a + 1", "", "s", "false"]);
    assert_eq!(
        column.definition,
        "integer GENERATED ALWAYS AS (a + 1) STORED"
    );
    let (column, _) = pg(&["g", "g", "integer", "false", "(a + 1)", "", "v", "false"]);
    assert_eq!(column.definition, "integer GENERATED ALWAYS AS (a + 1)");

    assert_eq!(
        join_statements([
            "CREATE TABLE a (x);\n".to_string(),
            " CREATE INDEX i ON a (x) ".to_string()
        ]),
        "CREATE TABLE a (x);\n\nCREATE INDEX i ON a (x);"
    );
}

#[test]
fn renders_bytes_as_hex_preview() {
    assert_eq!(render_bytes(&[]), "<0 bytes>");
    assert_eq!(render_bytes(&[0xde, 0xad, 0xbe, 0xef]), "0xdeadbeef");
    assert_eq!(render_bytes(&[0; 16]), format!("0x{}", "00".repeat(16)));
    assert_eq!(
        render_bytes(&[0xab; 1000]),
        format!("0x{}… <1000 bytes>", "ab".repeat(16))
    );
}

#[test]
fn renders_postgres_bytea() {
    assert_eq!(render_pg_bytea("\\x"), "<0 bytes>");
    assert_eq!(render_pg_bytea("\\xDEADbeef"), "0xdeadbeef");
    assert_eq!(
        render_pg_bytea(&format!("\\x{}", "0f".repeat(40))),
        format!("0x{}… <40 bytes>", "0f".repeat(16))
    );
    assert_eq!(render_pg_bytea("\\xzz"), "\\xzz");
    assert_eq!(render_pg_bytea("\\xabc"), "\\xabc");
    assert_eq!(render_pg_bytea("\\xé1"), "\\xé1");
    assert_eq!(render_pg_bytea("escape\\000"), "escape\\000");
}

#[test]
fn pagination_math() {
    assert_eq!(page_count(0, 100), 1);
    assert_eq!(page_count(1, 100), 1);
    assert_eq!(page_count(100, 100), 1);
    assert_eq!(page_count(101, 100), 2);
    assert_eq!(page_count(250, 100), 3);
    assert_eq!(page_count(5, 0), 5);
    assert_eq!(page_count(u64::MAX, 1), u64::MAX);

    assert_eq!(page_number(0, 100), 1);
    assert_eq!(page_number(99, 100), 1);
    assert_eq!(page_number(100, 100), 2);
    assert_eq!(page_number(200, 100), 3);
    assert_eq!(page_number(7, 0), 8);

    assert_eq!(last_page_offset(0, 100), 0);
    assert_eq!(last_page_offset(100, 100), 0);
    assert_eq!(last_page_offset(101, 100), 100);
    assert_eq!(last_page_offset(250, 100), 200);
    assert_eq!(last_page_offset(300, 100), 200);
}

#[test]
fn splits_extra_row_into_has_more() {
    assert_eq!(split_page(vec![1, 2, 3], 2), (vec![1, 2], true));
    assert_eq!(split_page(vec![1, 2], 2), (vec![1, 2], false));
    assert_eq!(split_page(vec![1], 2), (vec![1], false));
    assert_eq!(split_page(Vec::<u8>::new(), 2), (vec![], false));
    assert_eq!(split_page(vec![1], 0), (vec![], true));
}

#[test]
fn coalesces_superseded_page_and_count_requests() {
    let fetch = |id| Request::FetchPage {
        id,
        schema: "s".into(),
        table: "t".into(),
        offset: 0,
        limit: 1,
    };
    let count = |id| Request::CountRows {
        id,
        schema: "s".into(),
        table: "t".into(),
    };
    let tables = |id| Request::ListTables {
        id,
        schema: "s".into(),
    };
    let batch = vec![
        fetch(1),
        tables(2),
        count(3),
        fetch(4),
        Request::ListSchemas { id: 5 },
        count(6),
        tables(7),
    ];
    let ids: Vec<u64> = coalesce(batch).iter().map(Request::id).collect();
    assert_eq!(ids, [2, 4, 5, 6, 7]);
    assert!(coalesce(Vec::new()).is_empty());
}

#[test]
fn sqlite_session_browses_database() {
    let (_dir, path) = seeded_sqlite();
    let session = Session::spawn(sqlite_connection(&path), None);
    exercise(&session, DbKind::Sqlite, "main");
}

#[test]
fn sqlite_lists_only_user_objects_and_schemas() {
    let (_dir, path) = seeded_sqlite();
    let session = Session::spawn(sqlite_connection(&path), None);
    assert!(
        matches!(wait(&session), Response::Connected { server_info } if server_info.starts_with("SQLite 3"))
    );
    let id = session.list_schemas();
    assert_eq!(
        wait_for(&session, id),
        Response::Schemas {
            id,
            schemas: vec!["main".to_string()]
        }
    );
    let id = session.list_tables("main");
    let Response::Tables { tables, .. } = wait_for(&session, id) else {
        panic!("expected tables");
    };
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            USERS, EMPTY, "indexed", "kinds", "no_pk", "pair", "slow", VIEW, WEIRD
        ]
    );
}

#[test]
fn sqlite_orders_by_primary_key_and_renders_values() {
    let (_dir, path) = seeded_sqlite();
    let session = Session::spawn(sqlite_connection(&path), None);

    let pair = page(&session, "main", "pair", 0, 2);
    let values: Vec<_> = pair.rows.iter().map(|r| r[2].clone().unwrap()).collect();
    assert_eq!(values, ["first", "second"]);
    assert!(pair.has_more);
    let rest = page(&session, "main", "pair", 2, 2);
    assert_eq!(rest.rows[0][2].as_deref(), Some("third"));
    assert!(!rest.has_more);

    let no_pk = page(&session, "main", "no_pk", 0, 100);
    let values: Vec<_> = no_pk.rows.iter().map(|r| r[0].clone().unwrap()).collect();
    assert_eq!(values, ["b", "a", "c"]);

    let kinds = page(&session, "main", "kinds", 0, 100);
    let types: Vec<&str> = kinds.columns.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(types, ["integer", "real", "text", "blob", ""]);
    assert_eq!(
        kinds.rows[0],
        [
            Some("-7".to_string()),
            Some("1.5".to_string()),
            Some("ю😀".to_string()),
            Some(format!("0x{}… <40 bytes>", "00".repeat(16))),
            None,
        ]
    );
    assert_eq!(
        kinds.rows[1],
        [
            Some("0".to_string()),
            Some("0".to_string()),
            Some(String::new()),
            Some("<0 bytes>".to_string()),
            None,
        ]
    );

    let none = page(&session, "main", USERS, 0, 0);
    assert!(none.rows.is_empty());
    assert!(none.has_more);
}

#[test]
fn sqlite_pages_carry_the_key_and_the_opaque_cells() {
    let (_dir, path) = seeded_sqlite();
    let session = Session::spawn(sqlite_connection(&path), None);
    let users = page(&session, "main", USERS, 0, 3);
    assert_eq!(users.primary_key, ["id"]);
    let mut opaque: Vec<_> = users.opaque.iter().copied().collect();
    opaque.sort();
    assert_eq!(opaque, [(0, 3), (1, 3), (2, 3)], "the BLOB column only");
    assert_eq!(page(&session, "main", "pair", 0, 3).primary_key, ["b", "a"]);
    assert!(page(&session, "main", "no_pk", 0, 3).primary_key.is_empty());
    assert!(page(&session, "main", VIEW, 0, 3).primary_key.is_empty());
}

#[test]
fn sqlite_updates_one_row_by_its_key() {
    let (_dir, path) = seeded_sqlite();
    let session = Session::spawn(sqlite_connection(&path), None);
    let value = |table: &str, column: &str, filter: &str| -> Option<String> {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                &format!("SELECT CAST({column} AS TEXT) FROM {table} WHERE {filter}"),
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let users = r#""Users""#;

    let text = "it's \"quoted\"\n\tand ; DROP TABLE x; --";
    update(
        &session,
        row_update(
            USERS,
            &[("id", "2")],
            &[("name", Some(text)), ("bio", Some("")), ("data", None)],
        ),
    )
    .unwrap();
    assert_eq!(value(users, "name", "id = 2").as_deref(), Some(text));
    assert_eq!(value(users, "bio", "id = 2").as_deref(), Some(""));
    assert_eq!(value(users, "data", "id = 2"), None);
    assert_eq!(
        value(users, "name", "id = 3").as_deref(),
        Some("user 3"),
        "other rows stay"
    );

    update(
        &session,
        row_update(USERS, &[("id", "2")], &[("id", Some("1000"))]),
    )
    .unwrap();
    assert_eq!(
        value(users, "typeof(id)", "id = 1000").as_deref(),
        Some("integer")
    );

    update(
        &session,
        row_update("pair", &[("b", "1"), ("a", "2")], &[("v", Some("changed"))]),
    )
    .unwrap();
    assert_eq!(
        value("pair", "v", "a = 2 AND b = 1").as_deref(),
        Some("changed")
    );
    assert_eq!(
        value("pair", "v", "a = 1 AND b = 1").as_deref(),
        Some("first")
    );

    let gone = update(
        &session,
        row_update(USERS, &[("id", "2")], &[("name", Some("x"))]),
    )
    .unwrap_err();
    assert!(gone.contains("not found"), "{gone}");
    script(&session, "INSERT INTO no_pk VALUES ('a')", 10).unwrap();
    let twice = update(
        &session,
        row_update("no_pk", &[("v", "a")], &[("v", Some("z"))]),
    )
    .unwrap_err();
    assert!(twice.contains("matches 2 rows"), "{twice}");
    assert_eq!(
        value("no_pk", "COUNT(*)", "v = 'a'").as_deref(),
        Some("2"),
        "rolled back"
    );

    let null = update(
        &session,
        row_update(USERS, &[("id", "3")], &[("name", None)]),
    )
    .unwrap_err();
    assert!(null.contains("NOT NULL"), "{null}");
    update(
        &session,
        row_update(USERS, &[("id", "3")], &[("name", Some("fine"))]),
    )
    .unwrap();
    assert_eq!(value(users, "name", "id = 3").as_deref(), Some("fine"));

    let first = page(&session, "main", USERS, 0, 3);
    assert_eq!(first.rows[1][1].as_deref(), Some("fine"));
}

#[test]
fn sqlite_edits_do_not_join_a_transaction_the_console_left_open() {
    let (_dir, path) = seeded_sqlite();
    let session = Session::spawn(sqlite_connection(&path), None);
    script(&session, "BEGIN", 10).unwrap();
    update(
        &session,
        row_update(USERS, &[("id", "1")], &[("name", Some("edited"))]),
    )
    .unwrap();
    script(&session, "ROLLBACK", 10).unwrap();
    let name: String = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(r#"SELECT name FROM "Users" WHERE id = 1"#, [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        name, "edited",
        "the console's ROLLBACK did not undo the edit"
    );
}

#[test]
fn sqlite_is_opened_read_only_for_queries() {
    let (_dir, path) = seeded_sqlite();
    let backend =
        SqliteBackend::open(path.to_str().unwrap(), Arc::default(), false, false).unwrap();
    let err = backend
        .conn
        .execute("DELETE FROM \"Users\"", [])
        .unwrap_err();
    assert!(err.to_string().contains("readonly"), "{err}");

    let scripts = SqliteBackend::open(path.to_str().unwrap(), Arc::default(), true, false).unwrap();
    assert_eq!(scripts.conn.execute("DELETE FROM no_pk", []).unwrap(), 3);
}

fn read_only(conn: Connection) -> Connection {
    Connection {
        read_only: true,
        ..conn
    }
}

fn sqlite_count(path: &std::path::Path, table: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn a_read_only_sqlite_session_browses_and_reads_but_refuses_writes() {
    let (_dir, path) = seeded_sqlite();
    let session = Session::spawn(read_only(sqlite_connection(&path)), None);
    assert!(matches!(wait(&session), Response::Connected { .. }));

    assert_eq!(count(&session, "main", USERS), 250);
    assert_eq!(page(&session, "main", "no_pk", 0, 10).rows.len(), 3);
    assert!(ddl(&session, "main", USERS).unwrap().sql.contains("CREATE"));
    let read = script(&session, "SELECT v FROM no_pk ORDER BY v; SELECT 1", 10).unwrap();
    assert_eq!(read.statements, 2);
    script(&session, "BEGIN; SELECT 1; COMMIT", 10).unwrap();

    for sql in [
        "DELETE FROM no_pk",
        "SELECT 1; INSERT INTO no_pk VALUES ('d')",
        "CREATE TABLE made (id INTEGER)",
        "PRAGMA query_only = OFF; DELETE FROM no_pk",
        "ATTACH DATABASE ':memory:' AS other",
        "VACUUM",
    ] {
        let refused = script(&session, sql, 10).unwrap_err();
        assert!(refused.contains("read-only"), "{sql}: {refused}");
        assert!(refused.contains("Nothing was run"), "{sql}: {refused}");
    }
    let named = script(&session, "SELECT 1; DELETE FROM no_pk", 10).unwrap_err();
    assert!(named.ends_with("\n\nDELETE FROM no_pk"), "{named}");
    assert_eq!(sqlite_count(&path, "no_pk"), 3);
    assert_eq!(sqlite_count(&path, "sqlite_master WHERE name = 'made'"), 0);

    let edit = update(
        &session,
        row_update(USERS, &[("id", "1")], &[("name", Some("edited"))]),
    )
    .unwrap_err();
    assert_eq!(edit, READ_ONLY_EDIT);
    assert_eq!(sqlite_count(&path, "\"Users\" WHERE name = 'edited'"), 0);

    script(&session, "SELECT 1", 10).unwrap();
}

#[test]
fn a_read_only_sqlite_session_reads_a_wal_database_another_connection_writes() {
    let (_dir, path) = seeded_sqlite();
    let writer = rusqlite::Connection::open(&path).unwrap();
    let mode: String = writer
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    writer
        .execute("INSERT INTO no_pk VALUES ('in the wal')", [])
        .unwrap();

    let session = Session::spawn(read_only(sqlite_connection(&path)), None);
    assert!(matches!(wait(&session), Response::Connected { .. }));
    assert_eq!(count(&session, "main", "no_pk"), 4);
    writer
        .execute("INSERT INTO no_pk VALUES ('later')", [])
        .unwrap();
    let read = script(&session, "SELECT COUNT(*) FROM no_pk", 10).unwrap();
    assert_eq!(read.rows, cells(&[&[Some("5")]]));
    assert!(script(&session, "DELETE FROM no_pk", 10).is_err());
    drop(session);
    drop(writer);
    assert_eq!(sqlite_count(&path, "no_pk"), 5);
}

#[test]
fn a_read_only_sqlite_backend_cannot_write_even_past_the_guard() {
    let (_dir, path) = seeded_sqlite();
    for writable in [true, false] {
        let mut backend =
            SqliteBackend::open(path.to_str().unwrap(), Arc::default(), writable, true).unwrap();
        for sql in [
            "DELETE FROM no_pk",
            "PRAGMA query_only = OFF; DELETE FROM no_pk",
            "CREATE TABLE made (id INTEGER)",
        ] {
            let err = backend.script(sql, 10).unwrap_err().message;
            assert!(err.contains("readonly"), "{writable} {sql}: {err}");
        }
        let err = backend
            .update_row(&row_update(
                USERS,
                &[("id", "1")],
                &[("name", Some("edited"))],
            ))
            .unwrap_err()
            .message;
        assert!(err.contains("readonly"), "{writable}: {err}");
        assert_eq!(backend.count("main", "no_pk").unwrap(), 3);
    }
    assert_eq!(sqlite_count(&path, "no_pk"), 3);
    assert_eq!(sqlite_count(&path, "\"Users\" WHERE name = 'edited'"), 0);
}

#[test]
fn only_read_only_connections_refuse_requests() {
    let (_dir, path) = seeded_sqlite();
    let writable = sqlite_connection(&path);
    let locked = read_only(writable.clone());
    let write = Request::RunScript {
        id: 1,
        sql: "DELETE FROM no_pk".into(),
        limit: 10,
    };
    let read = Request::RunScript {
        id: 2,
        sql: "SELECT * FROM no_pk".into(),
        limit: 10,
    };
    let edit = Request::UpdateRow {
        id: 3,
        update: row_update(USERS, &[("id", "1")], &[("name", Some("x"))]),
    };
    let browse = [
        Request::ListSchemas { id: 4 },
        Request::CountRows {
            id: 5,
            schema: "main".into(),
            table: USERS.into(),
        },
    ];
    for request in [&write, &read, &edit].into_iter().chain(&browse) {
        assert_eq!(refusal(&writable, request), None, "{request:?}");
    }
    assert!(refusal(&locked, &write).unwrap().contains("read-only"));
    assert_eq!(refusal(&locked, &edit).as_deref(), Some(READ_ONLY_EDIT));
    for request in std::iter::once(&read).chain(&browse) {
        assert_eq!(refusal(&locked, request), None, "{request:?}");
    }
}

#[test]
fn sqlite_describes_tables_with_their_indexes_and_triggers() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    let create = "CREATE TABLE described (\n  id INTEGER PRIMARY KEY,\n  \
                  name TEXT NOT NULL DEFAULT 'x',\n  n NUMERIC DEFAULT (1 + 2),\n  \
                  g INTEGER GENERATED ALWAYS AS (id * 2) VIRTUAL,\n  plain\n)";
    let index = "CREATE INDEX described_name ON described (name)";
    let trigger = "CREATE TRIGGER described_touch AFTER UPDATE ON described BEGIN SELECT 1; END";
    script(&session, &format!("{create};\n{index};\n{trigger};"), 10).unwrap();

    let described = ddl(&session, "main", "described").unwrap();
    assert_eq!(
        described.sql,
        format!("{create};\n\n{index};\n\n{trigger};")
    );
    assert_eq!(
        definitions(&described),
        [
            ("id", "INTEGER PRIMARY KEY"),
            ("name", "TEXT NOT NULL DEFAULT 'x'"),
            ("n", "NUMERIC DEFAULT 1 + 2"),
            ("g", "INTEGER GENERATED ALWAYS VIRTUAL"),
            ("plain", ""),
        ]
    );

    let pair = ddl(&session, "main", "pair").unwrap();
    assert!(pair.sql.starts_with("CREATE TABLE pair ("), "{}", pair.sql);
    assert_eq!(
        definitions(&pair),
        [
            ("a", "INTEGER NOT NULL PRIMARY KEY"),
            ("b", "INTEGER NOT NULL PRIMARY KEY"),
            ("v", "TEXT"),
        ]
    );
    let indexed = ddl(&session, "main", "indexed").unwrap();
    assert_eq!(indexed.sql, "CREATE TABLE indexed (v TEXT UNIQUE);");

    let slow = ddl(&session, "main", "slow").unwrap();
    assert!(slow.sql.starts_with("CREATE VIEW slow AS"), "{}", slow.sql);
    assert_eq!(definitions(&slow), [("x", "")]);

    let missing = ddl(&session, "main", "nope").unwrap_err();
    assert_eq!(missing, "main.nope has no definition to show");
    assert_eq!(count(&session, "main", "pair"), 3, "the session goes on");
}

#[test]
fn sqlite_script_stops_reading_a_query_at_the_limit() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    let started = Instant::now();
    let result = script(&session, "SELECT x, x * 2 AS twice FROM slow", 5).unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(result.truncated);
    assert_eq!(result.rows.len(), 5);
    assert_eq!(
        result.rows[4],
        [Some("5".to_string()), Some("10".to_string())]
    );
    assert!(result.elapsed <= started.elapsed());
}

#[test]
fn sqlite_script_finishes_writes_beyond_the_limit() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    let result = script(
        &session,
        "INSERT INTO no_pk SELECT name FROM \"Users\" ORDER BY id RETURNING v",
        2,
    )
    .unwrap();
    assert!(result.truncated);
    assert_eq!(result.rows, cells(&[&[Some("user 1")], &[Some("user 2")]]));
    assert_eq!(result.affected, 250);
    assert_eq!(count(&session, "main", "no_pk"), 253);
}

#[test]
fn sqlite_script_keeps_the_statements_before_an_error() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    let failed = script(
        &session,
        "INSERT INTO no_pk VALUES ('kept');\n\
         SELECT * FROM missing;\n\
         INSERT INTO no_pk VALUES ('never')",
        10,
    );
    assert!(failed.unwrap_err().contains("missing"));
    assert_eq!(count(&session, "main", "no_pk"), 4);

    let blank = script(&session, " \n -- nothing to do\n ; ", 10).unwrap();
    assert_eq!(blank, ScriptResult::default().with_elapsed(blank.elapsed));
    assert!(script(&session, "SELECT ?", 10).is_err());
}

#[test]
fn sqlite_script_keeps_its_session_between_runs() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    script(&session, "CREATE TEMP TABLE scratch (v); BEGIN", 10).unwrap();
    script(&session, "INSERT INTO scratch VALUES (1), (2)", 10).unwrap();
    script(&session, "DELETE FROM no_pk", 10).unwrap();
    assert_eq!(count(&session, "main", "no_pk"), 3, "not committed yet");
    let result = script(&session, "COMMIT; SELECT COUNT(*) FROM scratch", 10).unwrap();
    assert_eq!(result.rows, cells(&[&[Some("2")]]));
    assert_eq!(count(&session, "main", "no_pk"), 0);
}

#[test]
fn running_script_does_not_block_browsing_and_can_be_cancelled() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    let slow = session.run_script("DELETE FROM no_pk; SELECT COUNT(*) FROM slow", 10);
    wait_until_running(&session.scripts.shared);

    assert_eq!(page(&session, "main", USERS, 0, 100).rows.len(), 100);
    assert_eq!(count(&session, "main", USERS), 250);
    assert!(session.scripts.shared.is_running(Some(slow)));

    let started = Instant::now();
    session.cancel_script(slow);
    session.cancel_script(slow);
    match wait(&session) {
        Response::Error { id, message } => {
            assert_eq!(id, Some(slow));
            assert!(message.contains("interrupted"), "{message}");
        }
        other => panic!("expected the cancelled script, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(count(&session, "main", "no_pk"), 0, "it is not undone");

    let next = script(&session, "SELECT 1 AS one", 10).unwrap();
    assert_eq!(next.rows, cells(&[&[Some("1")]]));
    session.cancel_script(slow);
    assert!(script(&session, "SELECT 2", 10).is_ok());
    assert_eq!(session.try_recv(), None);
}

#[test]
fn cancelling_a_queued_script_skips_it() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    let running = session.run_script("SELECT COUNT(*) FROM slow", 10);
    wait_until_running(&session.scripts.shared);
    let queued = session.run_script("DELETE FROM no_pk", 10);
    let last = session.run_script("SELECT COUNT(*) FROM no_pk", 10);
    session.cancel_script(queued);
    session.cancel_script(running);

    let mut answers = Vec::new();
    for _ in 0..3 {
        answers.push(wait(&session));
    }
    assert!(matches!(&answers[0], Response::Error { id, .. } if *id == Some(running)));
    assert_eq!(
        answers[1],
        Response::Error {
            id: Some(queued),
            message: "cancelled before it started".to_string()
        }
    );
    match &answers[2] {
        Response::Script { id, result } => {
            assert_eq!(*id, last);
            assert_eq!(result.rows, cells(&[&[Some("3")]]));
        }
        other => panic!("expected the last script, got {other:?}"),
    }
}

#[test]
fn dropping_session_interrupts_a_running_script() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    session.run_script("SELECT COUNT(*) FROM slow", 10);
    let shared = Arc::clone(&session.scripts.shared);
    wait_until_running(&shared);
    let started = Instant::now();
    drop(session);
    assert!(started.elapsed() < Duration::from_millis(500));
    wait_for_worker_exit(&shared);
}

#[test]
fn sqlite_missing_file_is_an_error_and_is_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.sqlite");
    let session = Session::spawn(sqlite_connection(&path), None);
    match wait(&session) {
        Response::Error { id: None, message } => assert!(message.contains("missing.sqlite")),
        other => panic!("expected a connection error, got {other:?}"),
    }
    assert!(!path.exists());
    session.list_schemas();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(session.try_recv(), None);

    let session = Session::spawn(connection(DbKind::Sqlite, None, "", ""), None);
    assert!(matches!(wait(&session), Response::Error { id: None, .. }));
}

#[test]
fn sqlite_rejects_files_that_are_not_databases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(
        &path,
        "definitely not a database, just some text\n".repeat(50),
    )
    .unwrap();
    let session = Session::spawn(sqlite_connection(&path), None);
    assert!(matches!(wait(&session), Response::Error { id: None, .. }));
    let session = Session::spawn(sqlite_connection(dir.path()), None);
    assert!(matches!(wait(&session), Response::Error { id: None, .. }));
}

fn wait_for_worker_exit(shared: &Arc<Shared>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(shared) > 1 {
        assert!(Instant::now() < deadline, "worker did not stop");
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_until_running(shared: &Arc<Shared>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !shared.is_running(None) {
        assert!(
            Instant::now() < deadline,
            "worker did not start the request"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn wait_until_idle(shared: &Arc<Shared>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while shared.is_running(None) {
        assert!(Instant::now() < deadline, "worker is still busy");
        thread::sleep(Duration::from_millis(1));
    }
}

fn connected_sqlite(path: &std::path::Path) -> Session {
    let session = Session::spawn(sqlite_connection(path), None);
    assert!(matches!(wait(&session), Response::Connected { .. }));
    session
}

#[test]
fn dropping_idle_session_stops_worker() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    let shared = [
        Arc::clone(&session.main.shared),
        Arc::clone(&session.counts.shared),
        Arc::clone(&session.scripts.shared),
    ];
    drop(session);
    shared.iter().for_each(wait_for_worker_exit);
}

#[test]
fn dropping_session_interrupts_running_queries() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    session.count_rows("main", "slow");
    session.fetch_page("main", "slow", u64::MAX / 2, 100);
    let shared = [
        Arc::clone(&session.main.shared),
        Arc::clone(&session.counts.shared),
    ];
    shared.iter().for_each(wait_until_running);
    thread::sleep(Duration::from_millis(50));
    assert_eq!(session.try_recv(), None);
    let started = Instant::now();
    drop(session);
    assert!(started.elapsed() < Duration::from_millis(500));
    shared.iter().for_each(wait_for_worker_exit);
}

#[test]
fn dropping_session_at_any_moment_stops_the_workers() {
    let (_dir, path) = seeded_sqlite();
    for round in 0..200 {
        let session = connected_sqlite(&path);
        session.fetch_page("main", "slow", u64::MAX / 2, 100);
        session.count_rows("main", "slow");
        let shared = [
            Arc::clone(&session.main.shared),
            Arc::clone(&session.counts.shared),
        ];
        thread::sleep(Duration::from_micros(round % 40 * 10));
        drop(session);
        let deadline = Instant::now() + Duration::from_secs(2);
        for shared in &shared {
            while Arc::strong_count(shared) > 1 {
                assert!(
                    Instant::now() < deadline,
                    "round {round}: a worker outlived its session"
                );
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

#[test]
fn running_count_does_not_block_other_requests() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    assert_eq!(page(&session, "main", USERS, 0, 100).rows.len(), 100);
    let slow = session.count_rows("main", "slow");
    wait_until_running(&session.counts.shared);

    let started = Instant::now();
    let other = session.fetch_page("main", EMPTY, 0, 100);
    let tables = session.list_tables("main");
    let schemas = session.list_schemas();
    let mut answered = Vec::new();
    while answered.len() < 3 {
        let response = wait(&session);
        assert_ne!(response.id(), Some(slow), "{response:?}");
        assert!(!matches!(response, Response::Error { .. }), "{response:?}");
        answered.push(response.id().unwrap());
    }
    assert_eq!(answered, [other, tables, schemas]);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(session.counts.shared.is_running(Some(slow)));
}

#[test]
fn newer_count_replaces_the_running_one() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    session.count_rows("main", "slow");
    wait_until_running(&session.counts.shared);

    let started = Instant::now();
    let newest = session.count_rows("main", USERS);
    let response = wait(&session);
    assert_eq!(
        response,
        Response::Count {
            id: newest,
            count: 250
        }
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    wait_until_idle(&session.counts.shared);
    assert_eq!(session.try_recv(), None);
}

#[test]
fn newer_page_replaces_the_running_one() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    session.fetch_page("main", "slow", u64::MAX / 2, 100);
    wait_until_running(&session.main.shared);
    let tables = session.list_tables("main");
    assert!(!session.main.shared.halted());

    let started = Instant::now();
    let newest = session.fetch_page("main", USERS, 200, 100);
    assert_eq!(wait(&session).id(), Some(tables));
    match wait(&session) {
        Response::Page { id, page } => {
            assert_eq!(id, newest);
            assert_eq!(page.rows.len(), 50);
        }
        other => panic!("expected the newest page, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    wait_until_idle(&session.main.shared);
    assert_eq!(session.try_recv(), None);
}

#[derive(Default)]
struct Flaky {
    cancels: u32,
    breaks: bool,
    broken: bool,
}

impl Backend for Flaky {
    fn server_info(&mut self) -> String {
        String::new()
    }

    fn schemas(&mut self) -> Result<Vec<String>, Failure> {
        Ok(Vec::new())
    }

    fn tables(&mut self, _: &str) -> Result<Vec<TableInfo>, Failure> {
        Ok(Vec::new())
    }

    fn rows(&mut self, _: &str, _: &str, _: u64, _: u32) -> Result<TableRows, Failure> {
        Ok(TableRows {
            columns: Vec::new(),
            rows: Vec::new(),
            primary_key: Vec::new(),
        })
    }

    fn count(&mut self, _: &str, _: &str) -> Result<u64, Failure> {
        if self.cancels == 0 {
            return Ok(7);
        }
        self.cancels -= 1;
        Err(Failure {
            message: "cancelled by someone".to_string(),
            cancelled: true,
        })
    }

    fn ddl(&mut self, _: &str, _: &str) -> Result<TableDdl, Failure> {
        Ok(TableDdl::default())
    }

    fn script(&mut self, _: &str, _: u32) -> Result<ScriptResult, Failure> {
        if self.breaks {
            self.broken = true;
            return Err("server closed the connection".into());
        }
        self.count("", "").map(|affected| ScriptResult {
            affected,
            ..ScriptResult::default()
        })
    }

    fn update_row(&mut self, _: &RowUpdate) -> Result<(), Failure> {
        Ok(())
    }

    fn is_broken(&mut self) -> bool {
        self.broken
    }

    fn canceller(&self) -> Option<Cancel> {
        None
    }
}

fn flaky_worker(path: &std::path::Path) -> Worker {
    let (_requests, request_rx) = mpsc::channel();
    let (responses, _response_rx) = mpsc::channel();
    Worker {
        conn: sqlite_connection(path),
        password: None,
        requests: request_rx,
        responses,
        shared: Arc::default(),
        announce: false,
        writable: true,
    }
}

fn flaky_script() -> Request {
    Request::RunScript {
        id: 9,
        sql: "SELECT 41 + 1".into(),
        limit: 10,
    }
}

fn serve_flaky(request: &Request, cancels: u32, aborted: bool) -> Response {
    let worker = flaky_worker(std::path::Path::new("unused"));
    assert!(worker.shared.begin(request));
    worker.shared.state().aborted = aborted;
    let mut backend: Option<Box<dyn Backend>> = Some(Box::new(Flaky {
        cancels,
        ..Flaky::default()
    }));
    worker.serve(&mut backend, request)
}

#[test]
fn stray_cancels_are_retried_but_not_forever() {
    let request = Request::CountRows {
        id: 9,
        schema: "s".into(),
        table: "t".into(),
    };
    assert_eq!(
        serve_flaky(&request, SPARE_RETRIES, false),
        Response::Count { id: 9, count: 7 }
    );
    let error = Response::Error {
        id: Some(9),
        message: "cancelled by someone".to_string(),
    };
    assert_eq!(serve_flaky(&request, SPARE_RETRIES + 1, false), error);
    assert_eq!(serve_flaky(&request, 1, true), error);
}

#[test]
fn scripts_are_never_run_twice() {
    let error = Response::Error {
        id: Some(9),
        message: "cancelled by someone".to_string(),
    };
    assert_eq!(serve_flaky(&flaky_script(), 1, false), error);
    assert!(matches!(
        serve_flaky(&flaky_script(), 0, false),
        Response::Script { id: 9, result } if result.affected == 7
    ));

    let worker = flaky_worker(std::path::Path::new("unused"));
    let mut backend: Option<Box<dyn Backend>> = Some(Box::new(Flaky {
        breaks: true,
        ..Flaky::default()
    }));
    match worker.serve(&mut backend, &flaky_script()) {
        Response::Error { id, message } => {
            assert_eq!(id, Some(9));
            assert!(
                message.starts_with("server closed the connection ("),
                "{message}"
            );
            assert!(message.contains("only in part"), "{message}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(backend.is_none(), "the next request reconnects");
}

#[test]
fn script_reconnects_before_running_on_a_dead_connection() {
    let (_dir, path) = seeded_sqlite();
    let worker = flaky_worker(&path);
    let mut backend: Option<Box<dyn Backend>> = Some(Box::new(Flaky {
        broken: true,
        ..Flaky::default()
    }));
    match worker.serve(&mut backend, &flaky_script()) {
        Response::Script { id: 9, result } => assert_eq!(result.rows, cells(&[&[Some("42")]])),
        other => panic!("expected a result from a fresh connection, got {other:?}"),
    }
}

#[test]
fn error_chain_appends_causes() {
    #[derive(Debug)]
    struct Outer(std::io::Error);
    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("error connecting to server")
        }
    }
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }
    let err = Outer(std::io::Error::other("connection refused"));
    assert_eq!(
        error_chain(&err),
        "error connecting to server: connection refused"
    );
    assert_eq!(error_chain(&err.0), "connection refused");
}

#[test]
fn connection_failures_become_errors() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    for kind in [DbKind::Postgres, DbKind::MySql] {
        let session = Session::spawn(
            connection(kind, Some(port), "nobody", "db"),
            Some("pw".into()),
        );
        match wait(&session) {
            Response::Error { id: None, message } => {
                assert!(message.to_lowercase().contains("refused"), "{message}")
            }
            other => panic!("expected a connection error, got {other:?}"),
        }
    }
}

#[test]
fn silent_server_times_out() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let started = Instant::now();
    let sessions = [DbKind::Postgres, DbKind::MySql].map(|kind| {
        Session::spawn(
            connection(kind, Some(port), "nobody", "db"),
            Some("pw".into()),
        )
    });
    for session in &sessions {
        match wait(session) {
            Response::Error { id: None, message } => {
                assert!(message.contains("timed out"), "{message}")
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
    }
    assert!(started.elapsed() >= CONNECT_TIMEOUT);
    assert!(started.elapsed() < CONNECT_TIMEOUT + Duration::from_secs(3));
    drop(listener);
}

fn env_port(name: &str) -> u16 {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("set {name} to the port of a throwaway server"))
        .parse()
        .unwrap()
}

fn env_password() -> String {
    std::env::var("COOPER_TEST_PASSWORD").unwrap_or_else(|_| "cooper".to_string())
}

fn env_tls(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|value| value == "1")
}

fn pg_batch(backend: &PgBackend, sql: &str) {
    backend
        .runtime
        .block_on(backend.client.batch_execute(sql))
        .unwrap();
}

fn result_cell(result: &ScriptResult, row: usize, column: usize) -> Option<String> {
    match result.full.get(&(row, column)) {
        Some(full) => Some(full.text.clone()),
        None => result.rows[row][column].clone(),
    }
}

fn exercise_updates(session: &Session, kind: DbKind, schema: &str) {
    let users = qualified(kind, schema, USERS).unwrap();
    let second = page(session, schema, USERS, 100, 100);
    assert_eq!(second.primary_key, ["id"]);
    assert!(second.opaque.contains(&(0, 3)), "the bytes are opaque");
    assert!(!second.opaque.contains(&(0, 1)));
    assert!(page(session, schema, VIEW, 0, 1).primary_key.is_empty());

    let edit = |id: &str, values: &[(&str, Option<&str>)]| RowUpdate {
        schema: schema.to_string(),
        table: USERS.to_string(),
        key: vec![("id".to_string(), id.to_string())],
        values: values
            .iter()
            .map(|(c, v)| (c.to_string(), v.map(str::to_string)))
            .collect(),
    };
    let read = |id: &str| {
        let result = script(
            session,
            &format!("SELECT name, bio FROM {users} WHERE id = {id}"),
            10,
        )
        .unwrap();
        (result.rows.len() == 1).then(|| (result_cell(&result, 0, 0), result_cell(&result, 0, 1)))
    };

    let text = "it's a \\back\\slash'; DROP TABLE x; -- ü😀\nline 2\t";
    update(session, edit("200", &[("name", Some(text)), ("bio", None)])).unwrap();
    assert_eq!(read("200"), Some((Some(text.to_string()), None)));
    assert_eq!(
        read("201").unwrap().0.as_deref(),
        Some("user 201"),
        "other rows stay"
    );

    update(session, edit("200", &[("bio", None)])).unwrap();

    update(session, edit("199", &[("id", Some("100199"))])).unwrap();
    assert!(read("199").is_none());
    assert_eq!(read("100199").unwrap().0.as_deref(), Some("user 199"));

    let gone = update(session, edit("100000", &[("bio", Some("x"))])).unwrap_err();
    assert!(gone.contains("not found"), "{gone}");
    assert!(update(session, edit("198", &[("name", None)])).is_err());
    assert_eq!(read("198").unwrap().0.as_deref(), Some("user 198"));

    script(session, "BEGIN", 10).unwrap();
    update(session, edit("197", &[("name", Some("edited"))])).unwrap();
    script(session, "ROLLBACK", 10).unwrap();
    assert_eq!(read("197").unwrap().0.as_deref(), Some("edited"));

    let page = page(session, schema, USERS, 100, 100);
    assert_eq!(page.rows[96][1].as_deref(), Some("edited"));
}

#[test]
#[ignore = "needs a throwaway PostgreSQL server"]
fn postgres_session_browses_database() {
    let conn = connection(
        DbKind::Postgres,
        Some(env_port("COOPER_TEST_PG_PORT")),
        "postgres",
        "postgres",
    );
    let password = env_password();
    let schema = "Odd \"Schema";

    let mut backend = PgBackend::connect(&conn, Some(&password)).unwrap();
    let quoted = quote_ident(DbKind::Postgres, schema);
    let mut statements = vec![
        format!("DROP SCHEMA IF EXISTS {quoted} CASCADE"),
        format!("CREATE SCHEMA {quoted}"),
    ];
    statements.extend(seed_statements(DbKind::Postgres, schema));
    statements.push(format!(
        "CREATE MATERIALIZED VIEW {quoted}.mat AS SELECT 1 AS one"
    ));
    statements.push(format!(
        "CREATE TABLE {quoted}.typed (id uuid PRIMARY KEY, j jsonb, a int[], ts timestamptz, n numeric(10,2), b boolean, e bytea)"
    ));
    statements.push(format!(
        "INSERT INTO {quoted}.typed VALUES ('00000000-0000-0000-0000-000000000001', '{{\"k\": [1, 2]}}', '{{1,2,3}}', '2024-01-02 03:04:05+00', 12.50, true, '')"
    ));
    statements.push(format!(
        "CREATE VIEW {quoted}.slow AS SELECT g FROM generate_series(1, 100000000000) g"
    ));
    statements.push(format!(
        "CREATE TABLE {quoted}.described (id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
         name varchar(20) NOT NULL DEFAULT 'x', owner_id integer REFERENCES {quoted}.\"Users\"(id), \
         total numeric(10,2) GENERATED ALWAYS AS (id * 2) STORED, \
         CONSTRAINT described_name_check CHECK (length(name::text) > 0))"
    ));
    statements.push(format!(
        "CREATE INDEX described_name ON {quoted}.described (name)"
    ));
    statements.push(format!(
        "CREATE TABLE {quoted}.parted (day date NOT NULL, v integer) PARTITION BY RANGE (day)"
    ));
    statements.push(format!(
        "CREATE TABLE {quoted}.parted_2024 PARTITION OF {quoted}.parted \
         FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')"
    ));
    pg_batch(&backend, &statements.join(";\n"));

    if let Some(expected) = env_tls("COOPER_TEST_PG_TLS") {
        let ssl = backend
            .strings(
                "SELECT ssl::text FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
                &[],
            )
            .unwrap();
        assert_eq!(ssl, [[expected.to_string()]]);
    }
    assert_eq!(
        backend.primary_key(schema, USERS).unwrap(),
        ["id".to_string()]
    );
    drop(backend);

    let session = Session::spawn(conn.clone(), Some(password.clone()));
    exercise(&session, DbKind::Postgres, schema);
    let described = ddl(&session, schema, "described").unwrap();
    assert_eq!(
        described.sql,
        format!(
            "CREATE TABLE {quoted}.described (\n    \
             id integer NOT NULL GENERATED ALWAYS AS IDENTITY,\n    \
             name character varying(20) NOT NULL DEFAULT 'x'::character varying,\n    \
             owner_id integer,\n    \
             total numeric(10,2) GENERATED ALWAYS AS (id * 2) STORED,\n    \
             CONSTRAINT described_pkey PRIMARY KEY (id),\n    \
             CONSTRAINT described_owner_id_fkey FOREIGN KEY (owner_id) \
             REFERENCES {quoted}.\"Users\"(id),\n    \
             CONSTRAINT described_name_check CHECK (length(name::text) > 0)\n);\n\n\
             CREATE INDEX described_name ON {quoted}.described USING btree (name);"
        )
    );
    assert_eq!(
        definitions(&described),
        [
            (
                "id",
                "integer NOT NULL GENERATED ALWAYS AS IDENTITY PRIMARY KEY"
            ),
            (
                "name",
                "character varying(20) NOT NULL DEFAULT 'x'::character varying"
            ),
            ("owner_id", "integer"),
            ("total", "numeric(10,2) GENERATED ALWAYS AS (id * 2) STORED"),
        ]
    );
    let parted = ddl(&session, schema, "parted").unwrap();
    assert_eq!(
        parted.sql,
        format!(
            "CREATE TABLE {quoted}.parted (\n    day date NOT NULL,\n    v integer\n)\n\
             PARTITION BY RANGE (day);"
        )
    );
    let partition = ddl(&session, schema, "parted_2024").unwrap();
    assert!(
        partition.sql.starts_with(&format!(
            "-- a partition of {quoted}.parted FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')\n\
             CREATE TABLE {quoted}.parted_2024 (\n"
        )),
        "{}",
        partition.sql
    );
    let mat = ddl(&session, schema, "mat").unwrap();
    assert_eq!(
        mat.sql,
        format!("CREATE MATERIALIZED VIEW {quoted}.mat AS\n SELECT 1 AS one;")
    );
    assert_eq!(definitions(&mat), [("one", "integer")]);
    let view = ddl(&session, schema, VIEW).unwrap();
    assert!(
        view.sql
            .starts_with(&format!("CREATE VIEW {quoted}.users_view AS\n SELECT ")),
        "{}",
        view.sql
    );

    let big = script(
        &session,
        "SELECT g, repeat('x', 200) FROM generate_series(1, 3000000) g",
        100,
    )
    .unwrap();
    assert_eq!(big.rows.len(), 100);
    assert!(big.truncated);
    script(
        &session,
        &format!(
            "SET search_path TO {quoted}; BEGIN; CREATE TEMP TABLE scratch AS SELECT 1 AS one"
        ),
        10,
    )
    .unwrap();
    let kept = script(
        &session,
        "SELECT one FROM scratch; SELECT count(*) FROM \"Users\"; COMMIT",
        10,
    )
    .unwrap();
    assert_eq!(kept.rows, cells(&[&[Some("250")]]));
    assert_eq!((kept.statements, kept.result_sets), (3, 2));

    let id = session.list_schemas();
    let Response::Schemas { schemas, .. } = wait_for(&session, id) else {
        panic!("expected schemas");
    };
    assert_eq!(schemas[0], "public");
    assert!(
        schemas
            .iter()
            .all(|s| !s.starts_with("pg_") && s != "information_schema"),
        "{schemas:?}"
    );

    let id = session.list_tables(schema);
    let Response::Tables { tables, .. } = wait_for(&session, id) else {
        panic!("expected tables");
    };
    let mat = tables.iter().find(|t| t.name == "mat").unwrap();
    assert_eq!(mat.kind, TableKind::MaterializedView);

    let typed = page(&session, schema, "typed", 0, 100);
    let types: Vec<&str> = typed.columns.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(
        types,
        [
            "uuid",
            "jsonb",
            "integer[]",
            "timestamp with time zone",
            "numeric(10,2)",
            "boolean",
            "bytea"
        ]
    );
    assert_eq!(
        typed.rows[0],
        [
            Some("00000000-0000-0000-0000-000000000001".to_string()),
            Some("{\"k\": [1, 2]}".to_string()),
            Some("{1,2,3}".to_string()),
            Some("2024-01-02 03:04:05+00".to_string()),
            Some("12.50".to_string()),
            Some("t".to_string()),
            Some("<0 bytes>".to_string()),
        ]
    );

    let mut killer = PgBackend::connect(&conn, Some(&password)).unwrap();
    pg_batch(
        &killer,
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = 'cooper' AND pid <> pg_backend_pid()",
    );
    thread::sleep(Duration::from_millis(200));
    assert_eq!(count(&session, schema, USERS), 250);
    assert_eq!(page(&session, schema, USERS, 200, 100).rows.len(), 50);
    let revived = script(&session, "SELECT 1 AS one", 10).unwrap();
    assert_eq!(revived.rows, cells(&[&[Some("1")]]));

    let wrong = Session::spawn(conn.clone(), Some("wrong password".into()));
    match wait(&wrong) {
        Response::Error { id: None, message } => assert!(message.contains("password"), "{message}"),
        other => panic!("expected an authentication error, got {other:?}"),
    }

    let missing = Session::spawn(conn.clone(), None);
    match wait(&missing) {
        Response::Error { id: None, message } => {
            assert!(message.contains("none is saved"), "{message}")
        }
        other => panic!("expected a missing password error, got {other:?}"),
    }

    let slow = Session::spawn(conn, Some(password));
    assert!(matches!(wait(&slow), Response::Connected { .. }));
    exercise_cancellation(&slow, DbKind::Postgres, schema);
    let shared = [
        Arc::clone(&slow.main.shared),
        Arc::clone(&slow.counts.shared),
    ];
    let started = Instant::now();
    drop(slow);
    assert!(started.elapsed() < Duration::from_secs(1));
    shared.iter().for_each(wait_for_worker_exit);
    thread::sleep(Duration::from_millis(300));
    let active = killer
        .strings(
            "SELECT count(*)::text FROM pg_stat_activity \
             WHERE application_name = 'cooper' AND state = 'active' AND pid <> pg_backend_pid()",
            &[],
        )
        .unwrap();
    assert_eq!(active, [["0".to_string()]]);

    exercise_updates(&session, DbKind::Postgres, schema);
    let typed = RowUpdate {
        schema: schema.to_string(),
        table: "typed".to_string(),
        key: vec![(
            "id".to_string(),
            "00000000-0000-0000-0000-000000000001".to_string(),
        )],
        values: [
            ("j", r#"{"k": 3}"#),
            ("a", "{4,5}"),
            ("ts", "2025-06-07 08:09:10+00"),
            ("n", "1.5"),
            ("b", "false"),
        ]
        .iter()
        .map(|(c, v)| (c.to_string(), Some(v.to_string())))
        .collect(),
    };
    update(&session, typed).unwrap();
    let typed = page(&session, schema, "typed", 0, 10);
    assert_eq!(
        typed.rows[0][1..6],
        ["{\"k\": 3}", "{4,5}", "2025-06-07 08:09:10+00", "1.50", "f"].map(|v| Some(v.to_string()))
    );
    assert!(typed.opaque.contains(&(0, 6)), "bytea is opaque");
}

fn exercise_cancellation(session: &Session, kind: DbKind, schema: &str) {
    let far = 10_000_000_000;
    let slow = qualified(kind, schema, "slow").unwrap();
    let stuck = session.run_script(&format!("SELECT COUNT(*) FROM {slow}"), 10);
    wait_until_running(&session.scripts.shared);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(page(session, schema, USERS, 0, 100).rows.len(), 100);
    assert!(session.scripts.shared.is_running(Some(stuck)));
    let started = Instant::now();
    session.cancel_script(stuck);
    match wait(session) {
        Response::Error { id, message } => {
            assert_eq!(id, Some(stuck));
            assert!(!message.is_empty());
        }
        other => panic!("expected the cancelled script, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(3));
    for _ in 0..5 {
        let one = script(session, "SELECT 1 AS one", 10).unwrap();
        assert_eq!(one.rows, cells(&[&[Some("1")]]));
    }
    let stuck = session.count_rows(schema, "slow");
    wait_until_running(&session.counts.shared);
    thread::sleep(Duration::from_millis(300));

    let started = Instant::now();
    assert_eq!(page(session, schema, USERS, 0, 100).rows.len(), 100);
    let id = session.list_tables(schema);
    assert!(matches!(wait_for(session, id), Response::Tables { .. }));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(session.counts.shared.is_running(Some(stuck)));

    let started = Instant::now();
    let newest = session.count_rows(schema, USERS);
    assert_eq!(
        wait(session),
        Response::Count {
            id: newest,
            count: 250
        }
    );
    assert!(started.elapsed() < Duration::from_secs(3));

    session.fetch_page(schema, "slow", far, 100);
    wait_until_running(&session.main.shared);
    thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    let newest = session.fetch_page(schema, USERS, 100, 100);
    match wait(session) {
        Response::Page { id, page } => {
            assert_eq!(id, newest);
            assert_eq!(page.rows.len(), 100);
        }
        other => panic!("expected the newest page, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(3));

    for _ in 0..20 {
        session.fetch_page(schema, "slow", far, 100);
        session.count_rows(schema, "slow");
        thread::sleep(Duration::from_millis(20));
    }
    let newest = session.fetch_page(schema, USERS, 200, 100);
    match wait(session) {
        Response::Page { id, page } => {
            assert_eq!(id, newest);
            assert_eq!(page.rows.len(), 50);
        }
        other => panic!("expected the newest page, got {other:?}"),
    }
    assert_eq!(count(session, schema, USERS), 250);
    wait_until_idle(&session.counts.shared);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(session.try_recv(), None);
    assert_eq!(count(session, schema, USERS), 250);

    session.count_rows(schema, "slow");
    session.fetch_page(schema, "slow", far, 100);
    let shared = [
        Arc::clone(&session.main.shared),
        Arc::clone(&session.counts.shared),
    ];
    shared.iter().for_each(wait_until_running);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(session.try_recv(), None);
}

#[test]
#[ignore = "needs a throwaway MySQL server"]
fn mysql_session_browses_database() {
    use mysql::prelude::Queryable;
    let schema = "cooper test-db";
    let mut conn = connection(
        DbKind::MySql,
        Some(env_port("COOPER_TEST_MYSQL_PORT")),
        "root",
        "",
    );
    let password = env_password();

    let mut backend = MyBackend::connect(&conn, Some(&password)).unwrap();
    let quoted = quote_ident(DbKind::MySql, schema);
    backend
        .conn
        .query_drop(format!("DROP DATABASE IF EXISTS {quoted}"))
        .unwrap();
    backend
        .conn
        .query_drop(format!("CREATE DATABASE {quoted} CHARACTER SET utf8mb4"))
        .unwrap();
    for statement in seed_statements(DbKind::MySql, schema) {
        backend.conn.query_drop(statement).unwrap();
    }
    backend
        .conn
        .query_drop(format!(
            "CREATE TABLE {quoted}.typed (id BIGINT UNSIGNED PRIMARY KEY, j JSON, d DECIMAL(10,2), ts DATETIME, bi BINARY(4), vb VARBINARY(8), t TEXT, f BIT(3), e ENUM('x','y'))"
        ))
        .unwrap();
    backend
        .conn
        .query_drop(format!(
            "INSERT INTO {quoted}.typed VALUES (18446744073709551615, '{{\"k\": 1}}', 12.50, '2024-01-02 03:04:05', X'00ff00ff', X'', 'tëxt', b'101', 'y')"
        ))
        .unwrap();
    backend
        .conn
        .query_drop(format!(
            "CREATE TABLE {quoted}.described (id INT AUTO_INCREMENT PRIMARY KEY, \
             name VARCHAR(20) NOT NULL DEFAULT 'it''s', code CHAR(3) DEFAULT '007', \
             n INT DEFAULT 5, made TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP \
             ON UPDATE CURRENT_TIMESTAMP, email VARCHAR(50) UNIQUE, \
             twice INT GENERATED ALWAYS AS (n * 2) STORED, KEY described_name (name))"
        ))
        .unwrap();
    if let Some(expected) = env_tls("COOPER_TEST_MYSQL_TLS") {
        let status = backend
            .strings("SHOW STATUS LIKE 'Ssl_cipher'", ())
            .unwrap();
        assert_eq!(!status[0][1].is_empty(), expected, "{status:?}");
    }
    drop(backend);

    let session = Session::spawn(conn.clone(), Some(password.clone()));
    exercise(&session, DbKind::MySql, schema);
    let described = ddl(&session, schema, "described").unwrap();
    let sql = &described.sql;
    assert!(sql.starts_with("CREATE TABLE `described` (\n"), "{sql}");
    assert!(sql.contains("  KEY `described_name` (`name`)\n"), "{sql}");
    assert!(sql.ends_with(';') && !sql.ends_with(";;"), "{sql}");
    let columns = definitions(&described);
    let of = |name: &str| columns.iter().find(|column| column.0 == name).unwrap().1;
    assert!(
        of("id").ends_with(" NOT NULL AUTO_INCREMENT PRIMARY KEY"),
        "{columns:?}"
    );
    assert_eq!(of("name"), "varchar(20) NOT NULL DEFAULT 'it''s'");
    assert_eq!(of("code"), "char(3) DEFAULT '007'");
    assert!(of("n").ends_with(" DEFAULT 5"), "{columns:?}");
    let made = of("made").to_uppercase();
    assert!(
        made.starts_with("TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP"),
        "{made}"
    );
    assert!(made.contains(" ON UPDATE CURRENT_TIMESTAMP"), "{made}");
    assert_eq!(of("email"), "varchar(50) UNIQUE");
    assert!(
        of("twice").ends_with(" GENERATED ALWAYS AS (`n` * 2) STORED"),
        "{columns:?}"
    );
    assert!(!of("twice").contains("DEFAULT"), "{columns:?}");
    let view = ddl(&session, schema, VIEW).unwrap();
    assert!(
        view.sql.contains(" VIEW `cooper test-db`.`users_view` AS "),
        "{}",
        view.sql
    );

    let id = session.list_schemas();
    let Response::Schemas { schemas, .. } = wait_for(&session, id) else {
        panic!("expected schemas");
    };
    let position = |name: &str| schemas.iter().position(|s| s == name).unwrap();
    assert!(
        position(schema) < position("information_schema"),
        "{schemas:?}"
    );
    assert!(position(schema) < position("mysql"));

    let typed = page(&session, schema, "typed", 0, 100);
    let types: Vec<&str> = typed.columns.iter().map(|c| c.type_name.as_str()).collect();
    assert!(types[0].starts_with("bigint") && types[0].ends_with("unsigned"));
    assert_eq!(types[2], "decimal(10,2)");
    assert_eq!(types[8], "enum('x','y')");
    let row = &typed.rows[0];
    assert_eq!(row[0].as_deref(), Some("18446744073709551615"));
    assert_eq!(row[1].as_deref(), Some("{\"k\": 1}"));
    assert_eq!(row[2].as_deref(), Some("12.50"));
    assert_eq!(row[3].as_deref(), Some("2024-01-02 03:04:05"));
    assert_eq!(row[4].as_deref(), Some("0x00ff00ff"));
    assert_eq!(row[5].as_deref(), Some("<0 bytes>"));
    assert_eq!(row[6].as_deref(), Some("tëxt"));
    assert_eq!(row[7].as_deref(), Some("0x05"));
    assert_eq!(row[8].as_deref(), Some("y"));

    let mut killer = MyBackend::connect(&conn, Some(&password)).unwrap();
    let others = killer
        .strings("SELECT ID FROM information_schema.PROCESSLIST WHERE ID <> CONNECTION_ID() AND USER = 'root'", ())
        .unwrap();
    assert!(!others.is_empty());
    for id in others.into_iter().flatten() {
        let _ = killer.conn.query_drop(format!("KILL {id}"));
    }
    thread::sleep(Duration::from_millis(200));
    assert_eq!(count(&session, schema, USERS), 250);
    assert_eq!(page(&session, schema, USERS, 200, 100).rows.len(), 50);
    let revived = script(&session, "SELECT 1 AS one", 10).unwrap();
    assert_eq!(revived.rows, cells(&[&[Some("1")]]));

    backend = MyBackend::connect(&conn, Some(&password)).unwrap();
    let users = qualified(DbKind::MySql, schema, USERS).unwrap();
    backend
        .conn
        .query_drop(format!(
            "CREATE VIEW {quoted}.slow AS SELECT a.id FROM {users} a, {users} b, {users} c, {users} d \
             WHERE SHA1(CONCAT(a.name, b.name, c.name, d.name)) <> ''"
        ))
        .unwrap();
    let slow = Session::spawn(conn.clone(), Some(password.clone()));
    assert!(matches!(wait(&slow), Response::Connected { .. }));
    exercise_cancellation(&slow, DbKind::MySql, schema);
    let shared = [
        Arc::clone(&slow.main.shared),
        Arc::clone(&slow.counts.shared),
    ];
    let started = Instant::now();
    drop(slow);
    assert!(started.elapsed() < Duration::from_secs(1));
    shared.iter().for_each(wait_for_worker_exit);
    thread::sleep(Duration::from_millis(300));
    let running = backend
        .strings(
            "SELECT COUNT(*) FROM information_schema.PROCESSLIST \
             WHERE ID <> CONNECTION_ID() AND INFO LIKE '%slow%'",
            (),
        )
        .unwrap();
    assert_eq!(running, [["0".to_string()]]);

    conn.database = schema.to_string();
    exercise_updates(&session, DbKind::MySql, schema);

    let scoped = Session::spawn(conn.clone(), Some(password));
    assert!(matches!(wait(&scoped), Response::Connected { .. }));
    let id = scoped.list_schemas();
    let Response::Schemas { schemas, .. } = wait_for(&scoped, id) else {
        panic!("expected schemas");
    };
    assert_eq!(schemas[0], schema);
    assert!(schemas.len() > 1);

    let wrong = Session::spawn(conn, Some("wrong password".into()));
    match wait(&wrong) {
        Response::Error { id: None, message } => assert!(message.contains("denied"), "{message}"),
        other => panic!("expected an authentication error, got {other:?}"),
    }
}

#[test]
fn background_cancel_wait_returns_at_once() {
    let grace = Duration::from_millis(300);
    let (_never_sent, sent) = mpsc::channel::<()>();
    let started = Instant::now();
    await_cancels(vec![sent], grace, true);
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "the caller waited"
    );

    let (_never_sent, sent) = mpsc::channel::<()>();
    let started = Instant::now();
    await_cancels(vec![sent], grace, false);
    assert!(
        started.elapsed() >= grace,
        "the inline wait gives the grace period"
    );
}

#[test]
fn dropping_a_background_drop_session_stops_its_workers() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    session.drop_in_background();
    session.run_script("SELECT COUNT(*) FROM slow", 10);
    let shared = Arc::clone(&session.scripts.shared);
    wait_until_running(&shared);
    let started = Instant::now();
    drop(session);
    assert!(started.elapsed() < Duration::from_millis(100));
    wait_for_worker_exit(&shared);
}

#[test]
fn a_session_dropped_in_the_foreground_again_waits_for_the_first_cancel() {
    let (_dir, path) = seeded_sqlite();
    for foreground in [false, true] {
        let session = connected_sqlite(&path);
        session.drop_in_background();
        if foreground {
            session.drop_in_foreground();
        }
        assert_eq!(session.drops_in_background(), !foreground);
        let shared = Arc::clone(&session.scripts.shared);
        let sent = Arc::new(AtomicBool::new(false));
        let (flag, lane) = (Arc::clone(&sent), Arc::downgrade(&shared));
        let cancel: Cancel = Arc::new(move || {
            thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::SeqCst);
            if let Some(lane) = lane.upgrade() {
                lane.state().running = None;
            }
        });
        *shared.cancel.lock().unwrap() = Some(cancel);
        shared.state().running = Some(Request::ListSchemas { id: 0 });
        drop(session);
        assert_eq!(
            sent.load(Ordering::SeqCst),
            foreground,
            "foreground drop {foreground}: the cancel was sent before the drop returned"
        );
        wait_for_worker_exit(&shared);
    }
}

#[test]
fn the_waker_runs_after_each_response() {
    let (_dir, path) = seeded_sqlite();
    let session = connected_sqlite(&path);
    let (woken, wakes) = mpsc::channel();
    session.set_waker(move || {
        let _ = woken.send(());
    });
    assert!(wakes.try_recv().is_err(), "no response yet");
    let id = session.list_schemas();
    wakes
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker woke the owner");
    assert!(matches!(session.try_recv(), Some(Response::Schemas { id: got, .. }) if got == id));
    let id = session.count_rows("main", USERS);
    wakes
        .recv_timeout(Duration::from_secs(10))
        .expect("the count lane wakes too");
    assert!(matches!(session.try_recv(), Some(Response::Count { id: got, .. }) if got == id));
}

#[test]
#[ignore = "needs a throwaway PostgreSQL server"]
fn postgres_read_only_session_cannot_write() {
    let conn = connection(
        DbKind::Postgres,
        Some(env_port("COOPER_TEST_PG_PORT")),
        "postgres",
        "postgres",
    );
    let password = env_password();
    let setup = PgBackend::connect(&conn, Some(&password)).unwrap();
    pg_batch(
        &setup,
        "DROP SCHEMA IF EXISTS cooper_ro CASCADE;
         CREATE SCHEMA cooper_ro;
         CREATE TABLE cooper_ro.t (id integer PRIMARY KEY, v text);
         INSERT INTO cooper_ro.t VALUES (1, 'one'), (2, 'two');
         CREATE FUNCTION cooper_ro.sneaky() RETURNS integer LANGUAGE sql
           AS 'INSERT INTO cooper_ro.t VALUES (9, ''sneaked'') RETURNING id';",
    );
    let rows = |setup: &mut PgBackend| setup.count("cooper_ro", "t").unwrap();

    let locked = read_only(conn.clone());
    let session = Session::spawn(locked.clone(), Some(password.clone()));
    assert!(matches!(wait(&session), Response::Connected { .. }));
    assert_eq!(count(&session, "cooper_ro", "t"), 2);
    assert_eq!(page(&session, "cooper_ro", "t", 0, 10).rows.len(), 2);
    let read = script(&session, "SELECT v FROM cooper_ro.t ORDER BY id", 10).unwrap();
    assert_eq!(read.rows, cells(&[&[Some("one")], &[Some("two")]]));
    script(&session, "BEGIN; SELECT 1; COMMIT", 10).unwrap();

    for sql in [
        "DELETE FROM cooper_ro.t",
        "CREATE TABLE cooper_ro.made (id integer)",
        "SET default_transaction_read_only = off; DELETE FROM cooper_ro.t",
        "BEGIN READ WRITE; DELETE FROM cooper_ro.t; COMMIT",
        "SELECT set_config('default_transaction_read_only', 'off', false)",
    ] {
        let refused = script(&session, sql, 10).unwrap_err();
        assert!(refused.contains("Nothing was run"), "{sql}: {refused}");
    }
    let sneaky = script(&session, "SELECT cooper_ro.sneaky()", 10).unwrap_err();
    assert!(sneaky.contains("read-only transaction"), "{sneaky}");
    let edit = update(
        &session,
        RowUpdate {
            schema: "cooper_ro".into(),
            table: "t".into(),
            key: vec![("id".into(), "1".into())],
            values: vec![("v".into(), Some("edited".into()))],
        },
    )
    .unwrap_err();
    assert_eq!(edit, READ_ONLY_EDIT);
    drop(session);

    let mut backend = PgBackend::connect(&locked, Some(&password)).unwrap();
    for sql in [
        "DELETE FROM cooper_ro.t",
        "INSERT INTO cooper_ro.t VALUES (3, 'three')",
        "CREATE TABLE cooper_ro.made (id integer)",
        "DROP TABLE cooper_ro.t",
        "TRUNCATE cooper_ro.t",
        "SELECT cooper_ro.sneaky()",
    ] {
        let err = backend.script(sql, 10).unwrap_err().message;
        assert!(err.contains("read-only transaction"), "{sql}: {err}");
    }
    assert_eq!(backend.count("cooper_ro", "t").unwrap(), 2);
    drop(backend);

    let mut setup = setup;
    assert_eq!(rows(&mut setup), 2);
    pg_batch(&setup, "DROP SCHEMA cooper_ro CASCADE");
}

#[test]
#[ignore = "needs a throwaway MySQL server"]
fn mysql_read_only_session_cannot_write() {
    use mysql::prelude::Queryable;
    let conn = connection(
        DbKind::MySql,
        Some(env_port("COOPER_TEST_MYSQL_PORT")),
        "root",
        "",
    );
    let password = env_password();
    let mut setup = MyBackend::connect(&conn, Some(&password)).unwrap();
    for sql in [
        "DROP DATABASE IF EXISTS cooper_ro",
        "CREATE DATABASE cooper_ro",
        "CREATE TABLE cooper_ro.t (id INT PRIMARY KEY, v VARCHAR(20))",
        "INSERT INTO cooper_ro.t VALUES (1, 'one'), (2, 'two')",
    ] {
        setup.conn.query_drop(sql).unwrap();
    }

    let locked = read_only(conn.clone());
    let session = Session::spawn(locked.clone(), Some(password.clone()));
    assert!(matches!(wait(&session), Response::Connected { .. }));
    assert_eq!(count(&session, "cooper_ro", "t"), 2);
    assert_eq!(page(&session, "cooper_ro", "t", 0, 10).rows.len(), 2);
    let read = script(&session, "SELECT v FROM cooper_ro.t ORDER BY id", 10).unwrap();
    assert_eq!(read.rows, cells(&[&[Some("one")], &[Some("two")]]));
    script(&session, "START TRANSACTION; SELECT 1; COMMIT", 10).unwrap();

    for sql in [
        "DELETE FROM cooper_ro.t",
        "CREATE TABLE cooper_ro.made (id INT)",
        "SET SESSION TRANSACTION READ WRITE; DELETE FROM cooper_ro.t",
        "SET @@session.transaction_read_only = 0",
        "START TRANSACTION READ WRITE; DELETE FROM cooper_ro.t; COMMIT",
    ] {
        let refused = script(&session, sql, 10).unwrap_err();
        assert!(refused.contains("Nothing was run"), "{sql}: {refused}");
    }
    let edit = update(
        &session,
        RowUpdate {
            schema: "cooper_ro".into(),
            table: "t".into(),
            key: vec![("id".into(), "1".into())],
            values: vec![("v".into(), Some("edited".into()))],
        },
    )
    .unwrap_err();
    assert_eq!(edit, READ_ONLY_EDIT);
    drop(session);

    let mut backend = MyBackend::connect(&locked, Some(&password)).unwrap();
    for sql in [
        "DELETE FROM cooper_ro.t",
        "INSERT INTO cooper_ro.t VALUES (3, 'three')",
        "UPDATE cooper_ro.t SET v = 'x'",
        "CREATE TABLE cooper_ro.made (id INT)",
        "DROP TABLE cooper_ro.t",
    ] {
        let err = backend.script(sql, 10).unwrap_err().message;
        assert!(err.to_lowercase().contains("read only"), "{sql}: {err}");
    }
    assert_eq!(backend.count("cooper_ro", "t").unwrap(), 2);
    drop(backend);

    assert_eq!(setup.count("cooper_ro", "t").unwrap(), 2);
    setup.conn.query_drop("DROP DATABASE cooper_ro").unwrap();
}
