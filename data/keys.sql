-- How Cooper finds a row to edit: one table per case.
-- Rebuild with data/build.sh.

-- Primary key -------------------------------------------------------------

CREATE TABLE users (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    email      TEXT,
    active     INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);
INSERT INTO users (id, name, email, active, created_at) VALUES
    (1, 'Alice',   'alice@example.com', 1, '2026-01-05 10:00:00'),
    (2, 'Bob',     NULL,                1, '2026-02-11 08:30:00'),
    (3, 'Carol',   'carol@example.com', 0, '2026-03-20 17:45:12'),
    (4, 'Дмитрий', 'dima@example.com',  1, '2026-04-01 09:00:00'),
    (5, '李雷',     NULL,                0, '2026-05-15 23:59:59');

CREATE TABLE order_items (
    order_id INTEGER NOT NULL,
    line     INTEGER NOT NULL,
    product  TEXT NOT NULL,
    qty      INTEGER NOT NULL,
    PRIMARY KEY (order_id, line)
);
INSERT INTO order_items VALUES
    (100, 1, 'keyboard', 1), (100, 2, 'mouse', 2),
    (101, 1, 'monitor', 1),  (102, 1, 'cable', 5), (102, 2, 'cable', 5);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT
) WITHOUT ROWID;
INSERT INTO settings VALUES ('theme', 'dark'), ('font', 'JetBrains Mono'), ('limit', NULL);

-- Unique NOT NULL index: "key: code (unique)" ------------------------------

CREATE TABLE countries (
    code TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    population INTEGER
);
INSERT INTO countries VALUES
    ('KZ', 'Kazakhstan', 20000000), ('DE', 'Germany', 84000000),
    ('JP', 'Japan', 124000000),     ('IS', 'Iceland', NULL);

-- Unique indexes that must NOT be used as a key: these fall back to rowid ----

CREATE TABLE nullable_unique (
    code TEXT UNIQUE,
    note TEXT
);
INSERT INTO nullable_unique VALUES ('a', 'first'), (NULL, 'no code'), (NULL, 'no code either');

CREATE TABLE partial_unique (
    sku     TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    title   TEXT
);
CREATE UNIQUE INDEX partial_unique_live ON partial_unique (sku) WHERE deleted = 0;
INSERT INTO partial_unique VALUES ('X1', 0, 'live'), ('X1', 1, 'deleted copy'), ('X2', 0, 'other');

CREATE TABLE expression_unique (
    email TEXT NOT NULL,
    name  TEXT
);
CREATE UNIQUE INDEX expression_unique_lower ON expression_unique (lower(email));
INSERT INTO expression_unique VALUES ('Ann@Example.com', 'Ann'), ('bob@example.com', 'Bob');

-- The index compares with NOCASE but the column with BINARY.
CREATE TABLE collation_unique (
    login TEXT NOT NULL,
    name  TEXT
);
CREATE UNIQUE INDEX collation_unique_login ON collation_unique (login COLLATE NOCASE);
INSERT INTO collation_unique VALUES ('admin', 'Administrator'), ('Guest', 'Visitor');

-- No key at all: found by rowid ("key: rowid") ------------------------------

CREATE TABLE event_log (
    at      TEXT NOT NULL,
    level   TEXT NOT NULL,
    message TEXT
);
INSERT INTO event_log VALUES
    ('2026-11-01 10:00:00', 'info',  'started'),
    ('2026-11-01 10:00:00', 'info',  'started'),   -- an exact duplicate
    ('2026-11-01 10:05:00', 'warn',  NULL),
    ('2026-11-01 10:07:30', 'error', 'disk full');

-- A Liquibase changelog: no primary key, no indexes.
CREATE TABLE databasechangelog (
    id            VARCHAR(255) NOT NULL,
    author        VARCHAR(255) NOT NULL,
    filename      VARCHAR(255) NOT NULL,
    dateexecuted  DATETIME NOT NULL,
    orderexecuted INT NOT NULL,
    exectype      VARCHAR(10) NOT NULL,
    md5sum        VARCHAR(35),
    description   VARCHAR(255),
    comments      VARCHAR(255),
    tag           VARCHAR(255),
    liquibase     VARCHAR(20),
    contexts      VARCHAR(255),
    labels        VARCHAR(255),
    deployment_id VARCHAR(10)
);
INSERT INTO databasechangelog VALUES
    ('1', 'nurbek', 'db/changelog/001-init.xml',  '2026-01-10 12:00:00', 1, 'EXECUTED', '9:3f1c2a', 'createTable tableName=users', '', NULL, '4.29.2', NULL, NULL, '6123400001'),
    ('2', 'nurbek', 'db/changelog/001-init.xml',  '2026-01-10 12:00:01', 2, 'EXECUTED', '9:a7b0e4', 'createTable tableName=orders', '', NULL, '4.29.2', NULL, NULL, '6123400001'),
    ('1', 'anna',   'db/changelog/002-index.xml', '2026-02-02 09:15:00', 3, 'EXECUTED', '9:0c9d11', 'createIndex indexName=idx_orders_user', '', 'v1.1', '4.29.2', 'prod', NULL, '6123400002'),
    ('3', 'nurbek', 'db/changelog/003-seed.xml',  '2026-03-03 18:30:00', 4, 'MARK_RAN', '9:55e6f0', 'sql', 'seed only on dev', NULL, '4.29.2', 'dev', 'seed', '6123400003');

-- Rowid-backed, but no column can be compared exactly: editing is refused ----

CREATE TABLE measurements (
    value  REAL,
    sample BLOB
);
INSERT INTO measurements VALUES (0.1, x'00ff'), (2.5, x'cafe'), (NULL, NULL);

CREATE TABLE loose (a, b);  -- untyped columns
INSERT INTO loose VALUES (1, 'one'), (2.5, x'01'), (NULL, 'three');

-- STRICT with ANY: v is left out of the rowid fingerprint.
CREATE TABLE strict_any (
    v ANY,
    w TEXT
) STRICT;
INSERT INTO strict_any VALUES (5, 'int'), ('five', 'text'), (5.5, 'real');

-- rowid, _rowid_ and oid are all taken by columns: "no key" -> choose columns

CREATE TABLE shadow (
    "rowid"   TEXT,
    "_rowid_" TEXT,
    oid       TEXT,
    v         TEXT
);
INSERT INTO shadow VALUES
    ('r1', 'x1', 'o1', 'first'),
    ('r2', 'x2', 'o2', NULL),
    ('r3', 'x3', 'o3', 'twin'),
    ('r3', 'x3', 'o3', 'twin');  -- identical rows: a key on all columns matches 2

-- Views are never editable ------------------------------------------------

CREATE VIEW active_users AS SELECT id, name, email FROM users WHERE active = 1;
CREATE VIEW changelog_by_author AS
    SELECT author, count(*) AS changesets FROM databasechangelog GROUP BY author;
