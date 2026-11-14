use super::{MAX_REFUSED, PREVIEW_CHARS, preview, refusal, refused, row_update, writes};
use crate::{connection::DbKind, db::RowUpdate};

const ALL: [DbKind; 3] = [DbKind::Postgres, DbKind::MySql, DbKind::Sqlite];
const PG: [DbKind; 1] = [DbKind::Postgres];
const MYSQL: [DbKind; 1] = [DbKind::MySql];
const SQLITE: [DbKind; 1] = [DbKind::Sqlite];
const PG_SQLITE: [DbKind; 2] = [DbKind::Postgres, DbKind::Sqlite];
const MYSQL_SQLITE: [DbKind; 2] = [DbKind::MySql, DbKind::Sqlite];
const PG_MYSQL: [DbKind; 2] = [DbKind::Postgres, DbKind::MySql];

fn verbs(sql: &str, kind: DbKind) -> Vec<(String, bool)> {
    writes(sql, kind)
        .into_iter()
        .map(|stmt| (stmt.verb, stmt.unbounded))
        .collect()
}

fn expected(labels: &[&str]) -> Vec<(String, bool)> {
    labels
        .iter()
        .map(|label| match label.strip_suffix('!') {
            Some(verb) => (verb.to_string(), true),
            None => (label.to_string(), false),
        })
        .collect()
}

fn check(kinds: &[DbKind], cases: &[(&str, &[&str])]) {
    for &kind in kinds {
        for (sql, labels) in cases {
            assert_eq!(verbs(sql, kind), expected(labels), "{kind:?}: {sql}");
        }
    }
}

fn reads(kinds: &[DbKind], sqls: &[&str]) {
    for &kind in kinds {
        for sql in sqls {
            assert_eq!(verbs(sql, kind), [], "{kind:?}: {sql}");
        }
    }
}

#[test]
fn plain_reads_and_writes() {
    reads(
        &ALL,
        &[
            "SELECT * FROM t",
            "select 1; SELECT 2;",
            "",
            "-- only a comment",
            ";;;",
        ],
    );
    check(
        &ALL,
        &[
            ("UPDATE t SET a = 1 WHERE id = 2", &["UPDATE"]),
            ("update t set a = 1", &["UPDATE!"]),
            ("dElEtE FrOm t", &["DELETE!"]),
            ("DELETE FROM t WHERE id = 1", &["DELETE"]),
            ("INSERT INTO t VALUES (1)", &["INSERT"]),
            (
                "SELECT 1; DELETE FROM t WHERE id = 1; SELECT 2",
                &["DELETE"],
            ),
            (
                "UPDATE a SET x = 1; UPDATE b SET y = 2 WHERE id = 1",
                &["UPDATE!", "UPDATE"],
            ),
        ],
    );
}

#[test]
fn comments_hide_only_what_the_dialect_hides() {
    reads(
        &ALL,
        &[
            "-- DELETE FROM t\nSELECT 1",
            "/* DELETE FROM t; */ SELECT 1",
            "SELECT 1 /* ; DELETE FROM t */",
        ],
    );
    reads(&PG, &["/* a /* b */ DELETE FROM t; */ SELECT 1"]);
    check(
        &MYSQL_SQLITE,
        &[("/* a /* b */ DELETE FROM t; */ SELECT 1", &["DELETE!"])],
    );
    reads(
        &MYSQL,
        &["SELECT 1 -- DELETE FROM t", "# DELETE FROM t\nSELECT 1"],
    );
    check(&MYSQL, &[("SELECT 1--1; DELETE FROM t", &["DELETE!"])]);
    reads(&PG_SQLITE, &["SELECT 1--1; DELETE FROM t"]);
    check(&PG, &[("SELECT 1 # 2; DELETE FROM t", &["DELETE!"])]);
    check(&MYSQL, &[("/*!50000 DELETE FROM t */", &["DELETE!"])]);
    reads(&PG, &["/*! DELETE FROM t */ SELECT 1"]);
    check(
        &ALL,
        &[("UPDATE t SET a = 1 -- WHERE id = 1", &["UPDATE!"])],
    );
}

#[test]
fn a_lone_carriage_return_ends_a_postgres_line_comment() {
    check(
        &PG,
        &[
            ("-- cleanup\rDELETE FROM orders", &["DELETE!"]),
            ("SELECT 1; -- note\rDELETE FROM t", &["DELETE!"]),
            ("-- x\r\nDELETE FROM t", &["DELETE!"]),
        ],
    );
    reads(
        &MYSQL_SQLITE,
        &["-- x\rDELETE FROM t", "SELECT 1; -- note\rDELETE FROM t"],
    );
    reads(&MYSQL, &["# x\rDELETE FROM t"]);
}

#[test]
fn quotes_hide_only_what_the_dialect_hides() {
    reads(
        &ALL,
        &["SELECT 'x; DELETE FROM t'", "SELECT 'it''s; DELETE FROM t'"],
    );
    reads(&PG_SQLITE, &["SELECT \"a;DELETE\" FROM t"]);
    reads(&MYSQL_SQLITE, &["SELECT `a;DELETE` FROM t"]);
    check(&PG, &[("SELECT `a;DELETE` FROM t", &["DELETE!"])]);
    reads(&SQLITE, &["SELECT [a;DELETE] FROM t"]);
    reads(&PG, &["SELECT E'it\\'s; DELETE FROM t'"]);
    check(&PG, &[("SELECT 'C:\\'; DELETE FROM t; --'", &["DELETE!"])]);
    check(&MYSQL, &[("SELECT 'it\\'s; DELETE FROM t'", &["DELETE!"])]);
    reads(&PG_MYSQL, &["SELECT * FROM users WHERE name = 'O\\'Brien'"]);
    check(
        &PG_MYSQL,
        &[(
            "UPDATE t SET a = 1 WHERE id = 1; SELECT 'x\\''; DELETE FROM t; -- '",
            &["UPDATE", "DELETE!"],
        )],
    );
    let both_ways = "SELECT 'x\\''; DELETE FROM t; -- '; UPDATE t SET a = 1";
    check(&PG, &[(both_ways, &["UPDATE!", "DELETE!"])]);
    check(&MYSQL, &[(both_ways, &["DELETE!", "UPDATE!"])]);
    check(
        &ALL,
        &[
            ("SELECT 'unterminated", &["UNTERMINATED"]),
            ("SELECT 1 /* never closed", &["UNTERMINATED"]),
        ],
    );
}

#[test]
fn dollar_quotes_are_postgres_only() {
    reads(
        &PG,
        &[
            "SELECT $$; DELETE FROM t; $$",
            "SELECT $fn$ DELETE FROM t; $fn$",
            "SELECT $1",
        ],
    );
    check(
        &PG,
        &[
            ("DO $$ BEGIN DELETE FROM t; END $$", &["DO"]),
            (
                "CREATE FUNCTION f() RETURNS void AS $body$ DELETE FROM t; $body$ LANGUAGE sql",
                &["CREATE"],
            ),
            ("SELECT a$b$ FROM t; DELETE FROM u; -- $b$", &["DELETE!"]),
        ],
    );
    check(
        &MYSQL,
        &[("SELECT $$; DELETE FROM t; $$", &["DELETE!", "UNKNOWN"])],
    );
}

#[test]
fn parentheses_and_with_queries() {
    reads(&ALL, &["(SELECT 1) UNION (SELECT 2)"]);
    check(&ALL, &[("((DELETE FROM t))", &["DELETE!"])]);
    reads(
        &PG,
        &[
            "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 5) \
             SELECT * FROM r",
            "WITH x AS MATERIALIZED (SELECT 1) SELECT * FROM x",
        ],
    );
    check(
        &PG,
        &[
            (
                "WITH x AS (SELECT id FROM t) DELETE FROM t WHERE id IN (SELECT id FROM x)",
                &["WITH … DELETE"],
            ),
            (
                "WITH x AS (SELECT 1) UPDATE t SET a = 1",
                &["WITH … UPDATE!"],
            ),
            (
                "WITH d AS (DELETE FROM t RETURNING *) SELECT count(*) FROM d",
                &["WITH … DELETE!"],
            ),
            (
                "WITH u AS (UPDATE t SET a = 1 WHERE id = 2 RETURNING *) SELECT * FROM u",
                &["WITH … UPDATE"],
            ),
            (
                "WITH i AS (INSERT INTO t VALUES (1) RETURNING id) SELECT id FROM i",
                &["WITH … INSERT"],
            ),
            (
                "WITH x AS (SELECT 1) SELECT * INTO new_t FROM x",
                &["SELECT … INTO"],
            ),
            (
                "WITH gone AS (WITH stale AS (SELECT id FROM s) DELETE FROM orders RETURNING *) \
                 SELECT count(*) FROM gone",
                &["WITH … DELETE!"],
            ),
            (
                "WITH u AS (WITH x AS (SELECT 1) UPDATE t SET a = 1 WHERE id = 2 RETURNING *) \
                 SELECT * FROM u",
                &["WITH … UPDATE"],
            ),
            (
                "EXPLAIN ANALYZE WITH d AS (WITH x AS (SELECT 1) DELETE FROM t RETURNING *) \
                 SELECT * FROM d",
                &["EXPLAIN ANALYZE WITH … DELETE!"],
            ),
        ],
    );
    reads(
        &PG,
        &["WITH a AS (WITH b AS (SELECT 1) SELECT * FROM b) SELECT * FROM a"],
    );
    check(
        &MYSQL_SQLITE,
        &[(
            "WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x",
            &["WITH … INSERT"],
        )],
    );
    reads(
        &ALL,
        &["WITH x AS (SELECT * FROM t FOR UPDATE) SELECT * FROM x"],
    );
}

#[test]
fn explain_runs_only_with_analyze() {
    reads(
        &PG,
        &[
            "EXPLAIN SELECT * FROM t",
            "EXPLAIN DELETE FROM t",
            "EXPLAIN ANALYZE SELECT * FROM t",
        ],
    );
    check(
        &PG,
        &[
            (
                "EXPLAIN ANALYZE DELETE FROM t",
                &["EXPLAIN ANALYZE DELETE!"],
            ),
            (
                "explain (analyze, buffers) update t set a = 1 where id = 1",
                &["EXPLAIN ANALYZE UPDATE"],
            ),
            (
                "EXPLAIN (ANALYZE false) DELETE FROM t WHERE id = 1",
                &["EXPLAIN ANALYZE DELETE"],
            ),
            (
                "EXPLAIN ANALYZE WITH d AS (DELETE FROM t RETURNING 1) SELECT * FROM d",
                &["EXPLAIN ANALYZE WITH … DELETE!"],
            ),
            (
                "EXPLAIN ANALYZE SELECT nextval('s')",
                &["EXPLAIN ANALYZE SELECT nextval()"],
            ),
        ],
    );
    reads(
        &MYSQL,
        &[
            "EXPLAIN ANALYZE SELECT 1",
            "DESCRIBE t",
            "DESC t",
            "EXPLAIN FORMAT=JSON DELETE FROM t",
        ],
    );
    reads(&SQLITE, &["EXPLAIN QUERY PLAN DELETE FROM t"]);
}

#[test]
fn selects_that_store_or_have_side_effects() {
    check(&PG, &[("SELECT * INTO backup FROM t", &["SELECT … INTO"])]);
    check(
        &MYSQL,
        &[("SELECT * FROM t INTO OUTFILE '/tmp/x'", &["SELECT … INTO"])],
    );
    reads(&ALL, &["SELECT * FROM t WHERE id = 1 FOR UPDATE"]);
    reads(&PG, &["SELECT * FROM t FOR NO KEY UPDATE SKIP LOCKED"]);
    check(
        &PG,
        &[
            ("SELECT nextval('seq')", &["SELECT nextval()"]),
            ("SELECT pg_catalog.setval('seq', 1)", &["SELECT setval()"]),
            (
                "SELECT pg_terminate_backend(42)",
                &["SELECT pg_terminate_backend()"],
            ),
            (
                "SELECT * FROM dblink('dbname=app', 'DELETE FROM orders RETURNING id') AS r(id int)",
                &["SELECT dblink()"],
            ),
            ("SELECT pg_notify('c', 'x')", &["SELECT pg_notify()"]),
            (
                "SELECT pg_create_logical_replication_slot('s', 'pgoutput')",
                &["SELECT pg_create_logical_replication_slot()"],
            ),
            (
                "SELECT lowrite(lo_open(42, 131072), 'x')",
                &["SELECT lowrite()"],
            ),
            ("SELECT pg_stat_reset()", &["SELECT pg_stat_reset()"]),
            ("SELECT \"nextval\"('s')", &["SELECT nextval()"]),
            ("SELECT pg_catalog.\"setval\"('s', 1)", &["SELECT setval()"]),
            (
                "DECLARE c CURSOR FOR SELECT nextval('s')",
                &["DECLARE nextval()"],
            ),
        ],
    );
    check(
        &MYSQL,
        &[
            ("SELECT `nextval`(s)", &["SELECT nextval()"]),
            ("SET @id = nextval(s)", &["SET nextval()"]),
        ],
    );
    check(
        &SQLITE,
        &[("SELECT [load_extension]('x')", &["SELECT load_extension()"])],
    );
    reads(
        &ALL,
        &[
            "SELECT replace(name, 'a', 'b') FROM t",
            "SELECT nextval FROM t",
            "VALUES (1), (2)",
        ],
    );
    reads(&PG, &["TABLE t"]);
}

#[test]
fn other_writing_verbs() {
    check(
        &PG,
        &[
            ("COPY t FROM '/tmp/x.csv'", &["COPY"]),
            ("COPY (SELECT 1) TO STDOUT", &["COPY"]),
            ("NOTIFY ch", &["NOTIFY"]),
            ("LOCK TABLE t IN ACCESS EXCLUSIVE MODE", &["LOCK"]),
            ("VACUUM FULL t", &["VACUUM"]),
            (
                "INSERT INTO t VALUES (1) ON CONFLICT (id) DO UPDATE SET a = 1",
                &["INSERT"],
            ),
        ],
    );
    check(
        &PG_MYSQL,
        &[
            ("CALL p()", &["CALL"]),
            ("EXEC p", &["EXEC"]),
            ("EXECUTE s", &["EXECUTE"]),
        ],
    );
    check(
        &MYSQL,
        &[
            ("LOCK TABLES t WRITE", &["LOCK"]),
            (
                "INSERT INTO t VALUES (1) ON DUPLICATE KEY UPDATE a = 1",
                &["INSERT"],
            ),
        ],
    );
    check(
        &MYSQL_SQLITE,
        &[("REPLACE INTO t VALUES (1)", &["REPLACE"])],
    );
    check(
        &SQLITE,
        &[
            ("INSERT OR REPLACE INTO t VALUES (1)", &["INSERT"]),
            ("VACUUM INTO '/tmp/c.db'", &["VACUUM"]),
        ],
    );
    check(
        &ALL,
        &[
            (
                "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = s.a",
                &["MERGE"],
            ),
            ("GRANT SELECT ON t TO u", &["GRANT"]),
            ("REVOKE ALL ON t FROM u", &["REVOKE"]),
            ("VACUUM", &["VACUUM"]),
            ("ANALYZE t", &["ANALYZE"]),
            ("TRUNCATE t", &["TRUNCATE"]),
            ("DROP TABLE t", &["DROP"]),
            ("CREATE TABLE t (id int)", &["CREATE"]),
            ("ALTER TABLE t ADD c int", &["ALTER"]),
            ("CREATE TEMP TABLE x AS SELECT 1", &["CREATE"]),
            ("FROBNICATE t", &["FROBNICATE"]),
            ("'x'", &["UNKNOWN"]),
        ],
    );
    check(&PG, &[("\"t\"", &["UNKNOWN"])]);
}

#[test]
fn pragmas_write_only_when_given_a_value_or_acting() {
    reads(
        &SQLITE,
        &[
            "PRAGMA table_info(t)",
            "PRAGMA user_version",
            "PRAGMA main.journal_mode",
        ],
    );
    check(
        &SQLITE,
        &[
            ("PRAGMA user_version = 3", &["PRAGMA"]),
            ("PRAGMA main.journal_mode=WAL", &["PRAGMA"]),
            ("PRAGMA optimize", &["PRAGMA"]),
            ("PRAGMA wal_checkpoint(TRUNCATE)", &["PRAGMA"]),
            ("PRAGMA journal_mode(WAL)", &["PRAGMA"]),
        ],
    );
}

#[test]
fn transactions_sessions_and_cursors() {
    reads(
        &ALL,
        &[
            "BEGIN; SAVEPOINT a; RELEASE SAVEPOINT a; ROLLBACK TO a; ROLLBACK; COMMIT",
            "SHOW TABLES",
        ],
    );
    reads(&PG, &["START TRANSACTION; END; ABORT"]);
    reads(
        &PG,
        &[
            "BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY",
            "BEGIN ISOLATION LEVEL SERIALIZABLE NOT DEFERRABLE; COMMIT",
            "BEGIN WORK",
        ],
    );
    reads(&SQLITE, &["BEGIN IMMEDIATE TRANSACTION; COMMIT"]);
    check(
        &MYSQL,
        &[
            (
                "BEGIN NOT ATOMIC DELETE FROM t; END",
                &["BEGIN NOT ATOMIC DELETE!"],
            ),
            (
                "BEGIN NOT ATOMIC UPDATE t SET a = 1; SELECT 1; END",
                &["BEGIN NOT ATOMIC UPDATE!"],
            ),
            (
                "BEGIN NOT ATOMIC SELECT 1; DELETE FROM t WHERE id = 1; END",
                &["DELETE"],
            ),
            (
                "BEGIN NOT ATOMIC IF 1 THEN SELECT 1; END IF; END",
                &["BEGIN NOT ATOMIC IF"],
            ),
            ("BEGIN SOMETHING", &["BEGIN"]),
        ],
    );
    reads(&MYSQL, &["START TRANSACTION", "UNLOCK TABLES", "USE shop"]);
    check(&MYSQL, &[("START REPLICA", &["START"])]);
    reads(
        &PG_MYSQL,
        &[
            "SET search_path TO app",
            "SET ROLE admin",
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "SET NAMES utf8mb4",
            "SET autocommit = 0",
        ],
    );
    check(
        &MYSQL,
        &[
            ("SET GLOBAL max_connections = 10", &["SET GLOBAL"]),
            ("SET @@global.sql_mode = ''", &["SET GLOBAL"]),
            ("SET PERSIST x = 1", &["SET PERSIST"]),
            ("SET PASSWORD = 'x'", &["SET PASSWORD"]),
            ("RESET MASTER", &["RESET"]),
        ],
    );
    reads(
        &PG,
        &[
            "RESET search_path",
            "DECLARE c CURSOR FOR SELECT 1; FETCH 10 FROM c; CLOSE c",
            "DECLARE c CURSOR FOR SELECT nextval FROM t",
            "LISTEN ch; UNLISTEN ch",
            "PREPARE p AS DELETE FROM t; DEALLOCATE p",
        ],
    );
}

#[test]
fn where_is_looked_for_at_the_statement_level() {
    check(
        &ALL,
        &[
            (
                "UPDATE t SET a = (SELECT b FROM u WHERE u.id = t.id)",
                &["UPDATE!"],
            ),
            ("UPDATE t SET note = 'where'", &["UPDATE!"]),
        ],
    );
    check(
        &PG,
        &[
            ("DELETE FROM t USING u WHERE t.id = u.id", &["DELETE"]),
            ("DELETE FROM t WHERE CURRENT OF c", &["DELETE"]),
        ],
    );
    check(
        &MYSQL,
        &[
            (
                "DELETE t FROM t JOIN u ON t.id = u.id WHERE u.x = 1",
                &["DELETE"],
            ),
            ("UPDATE t SET a = 1 ORDER BY id LIMIT 1", &["UPDATE!"]),
        ],
    );
}

#[test]
fn previews_are_one_line_without_leading_comments() {
    let text = |sql: &str| writes(sql, DbKind::Postgres).remove(0).text;
    assert_eq!(
        text("UPDATE t\n   SET a = 1\n\tWHERE id = 2"),
        "UPDATE t SET a = 1 WHERE id = 2"
    );
    assert_eq!(text("-- note\nDELETE FROM t"), "DELETE FROM t");

    let long = format!("UPDATE t SET a = '{}'", "x".repeat(300));
    let cut = text(&long);
    assert_eq!(cut.chars().count(), PREVIEW_CHARS);
    assert!(cut.ends_with('…'), "{cut}");

    let cyrillic = format!("UPDATE t SET name = '{}'", "Привет ".repeat(40));
    let cut = preview(&cyrillic);
    assert_eq!(cut.chars().count(), PREVIEW_CHARS);
    assert!(
        cut.starts_with("UPDATE t SET name = 'Привет Привет"),
        "{cut}"
    );
    assert_eq!(preview("short"), "short");
}

#[test]
fn a_row_update_reads_as_the_sql_that_saves_it() {
    let update = RowUpdate {
        schema: "main".into(),
        table: "customers".into(),
        key: vec![("id".into(), "2".into())],
        values: vec![
            ("name".into(), Some("O'Brien".into())),
            ("email".into(), None),
        ],
    };
    let stmt = row_update(DbKind::Sqlite, &update).unwrap();
    assert_eq!(stmt.verb, "UPDATE");
    assert!(!stmt.unbounded);
    assert_eq!(
        stmt.text,
        "UPDATE \"main\".\"customers\" SET \"name\" = 'O''Brien', \"email\" = NULL \
         WHERE \"id\" = '2'"
    );
    assert!(
        row_update(DbKind::MySql, &update)
            .unwrap()
            .text
            .starts_with("UPDATE `main`.`customers` SET `name`"),
    );

    let bare = RowUpdate {
        schema: String::new(),
        ..update
    };
    assert!(
        row_update(DbKind::Sqlite, &bare).is_err(),
        "an edit the save would reject is not previewed"
    );
}

fn refused_verbs(sql: &str, kind: DbKind) -> Vec<String> {
    refused(sql, kind)
        .into_iter()
        .map(|stmt| stmt.verb)
        .collect()
}

#[test]
fn a_read_only_connection_refuses_everything_that_asks_to_confirm() {
    let scripts = [
        "UPDATE t SET a = 1",
        "SELECT 1; DELETE FROM t WHERE id = 1; SELECT 2",
        "WITH gone AS (DELETE FROM t RETURNING *) SELECT * FROM gone",
        "SELECT nextval('s')",
        "CREATE TABLE t (id int)",
        "EXPLAIN ANALYZE DELETE FROM t",
        "CALL p()",
        "SELECT 'unterminated",
        "frobnicate",
    ];
    for kind in ALL {
        for sql in scripts {
            assert_eq!(refused(sql, kind), writes(sql, kind), "{kind:?}: {sql}");
            assert!(!refused(sql, kind).is_empty(), "{kind:?}: {sql}");
        }
    }
}

#[test]
fn a_read_only_connection_runs_plain_reads() {
    for kind in ALL {
        for sql in [
            "SELECT * FROM t",
            "select 1; SELECT 2;",
            "",
            "-- only a comment",
            "EXPLAIN SELECT * FROM t",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "BEGIN; SELECT 1; COMMIT",
            "START TRANSACTION; SELECT 1; ROLLBACK",
            "BEGIN TRANSACTION READ ONLY; SELECT 1; COMMIT",
            "START TRANSACTION READ ONLY",
            "SET search_path = app",
            "SELECT read_only, 'READ WRITE' FROM flags",
            "SELECT * FROM t WHERE note = 'SET transaction_read_only = off'",
        ] {
            assert_eq!(refused_verbs(sql, kind), [] as [&str; 0], "{kind:?}: {sql}");
            assert_eq!(refusal(sql, kind), None, "{kind:?}: {sql}");
        }
    }
    for sql in [
        "SHOW search_path",
        "RESET search_path",
        "SET LOCAL work_mem = '1MB'",
    ] {
        assert_eq!(
            refused_verbs(sql, DbKind::Postgres),
            [] as [&str; 0],
            "{sql}"
        );
    }
    for sql in ["SHOW TABLES", "SET @x = 1", "USE shop", "SET NAMES utf8mb4"] {
        assert_eq!(refused_verbs(sql, DbKind::MySql), [] as [&str; 0], "{sql}");
    }
    for sql in ["PRAGMA table_info(t)", "PRAGMA query_only"] {
        assert_eq!(refused_verbs(sql, DbKind::Sqlite), [] as [&str; 0], "{sql}");
    }
}

#[test]
fn statements_that_lift_the_read_only_mode_are_refused_only_when_read_only() {
    let postgres: [(&str, &str); 14] = [
        ("SET default_transaction_read_only = off", "SET"),
        ("set DEFAULT_TRANSACTION_READ_ONLY to off", "SET"),
        ("SET \"default_transaction_read_only\" = off", "SET"),
        ("SET LOCAL transaction_read_only = off", "SET"),
        (
            "SET SESSION default_transaction_read_only TO DEFAULT",
            "SET",
        ),
        ("SET TRANSACTION READ WRITE", "SET"),
        (
            "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE",
            "SET",
        ),
        ("RESET default_transaction_read_only", "RESET"),
        ("RESET ALL", "RESET"),
        ("BEGIN READ WRITE", "BEGIN"),
        (
            "BEGIN ISOLATION LEVEL SERIALIZABLE, READ /* x */ WRITE",
            "BEGIN",
        ),
        ("START TRANSACTION READ WRITE", "START"),
        (
            "SELECT set_config('default_transaction_read_only', 'off', false)",
            "set_config()",
        ),
        (
            "SELECT pg_catalog.\"set_config\" ('x', 'y', true)",
            "set_config()",
        ),
    ];
    for (sql, verb) in postgres {
        assert_eq!(refused_verbs(sql, DbKind::Postgres), [verb], "{sql}");
        assert_eq!(
            verbs(sql, DbKind::Postgres),
            [],
            "{sql}: no write to confirm"
        );
    }
    let mysql: [(&str, &str); 7] = [
        ("SET SESSION TRANSACTION READ WRITE", "SET"),
        ("SET TRANSACTION READ WRITE", "SET"),
        ("SET SESSION transaction_read_only = OFF", "SET"),
        ("SET @@session.transaction_read_only = 0", "SET"),
        ("SET @@tx_read_only = 0", "SET"),
        ("SET `transaction_read_only` = 0", "SET"),
        ("START TRANSACTION READ WRITE", "START"),
    ];
    for (sql, verb) in mysql {
        assert_eq!(refused_verbs(sql, DbKind::MySql), [verb], "{sql}");
        assert_eq!(verbs(sql, DbKind::MySql), [], "{sql}: no write to confirm");
    }
    for sql in ["PRAGMA query_only = OFF", "PRAGMA query_only(0)"] {
        assert_eq!(refused_verbs(sql, DbKind::Sqlite), ["PRAGMA"], "{sql}");
    }
    assert_eq!(
        refused_verbs(
            "SELECT 1; SET default_transaction_read_only = off; DELETE FROM t",
            DbKind::Postgres
        ),
        ["SET", "DELETE"]
    );
}

#[test]
fn the_refusal_names_the_statements_that_were_not_plain_reads() {
    let one = refusal(
        "SELECT 1;\nUPDATE t\n   SET a = 1\n WHERE id = 2;",
        DbKind::Postgres,
    )
    .unwrap();
    assert_eq!(
        one,
        "Nothing was run: this connection is read-only, and the script has a statement \
         that is not a plain read:\n\nUPDATE t SET a = 1 WHERE id = 2"
    );

    let many: String = (0..MAX_REFUSED + 2)
        .map(|i| format!("DELETE FROM t WHERE id = {i};"))
        .collect();
    let text = refusal(&many, DbKind::Sqlite).unwrap();
    assert!(
        text.contains(&format!(
            "the script has {} statements that are not plain reads:",
            MAX_REFUSED + 2
        )),
        "{text}"
    );
    assert!(text.contains("DELETE FROM t WHERE id = 0"), "{text}");
    assert!(
        text.contains(&format!("DELETE FROM t WHERE id = {}", MAX_REFUSED - 1)),
        "{text}"
    );
    assert!(
        !text.contains(&format!("DELETE FROM t WHERE id = {MAX_REFUSED}")),
        "{text}"
    );
    assert!(text.ends_with("…and 2 more"), "{text}");
}
