-- Values that are hard to show or edit, plus enough rows for paging.
-- Rebuild with data/build.sh.

CREATE TABLE products (
    id          INTEGER PRIMARY KEY,
    sku         TEXT NOT NULL UNIQUE,
    title       TEXT NOT NULL,
    price       NUMERIC(10, 2) NOT NULL,
    weight      REAL,
    in_stock    BOOLEAN NOT NULL DEFAULT 1,
    attributes  TEXT,             -- JSON as text
    released_on DATE,
    updated_at  DATETIME
);
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000)
INSERT INTO products (id, sku, title, price, weight, in_stock, attributes, released_on, updated_at)
SELECT
    i,
    printf('SKU-%05d', i),
    printf('Product %d', i),
    round((i * 7919 % 100000) / 100.0, 2),
    CASE WHEN i % 10 = 0 THEN NULL ELSE (i % 97) / 3.0 END,
    i % 4 <> 0,
    json_object('color', CASE i % 3 WHEN 0 THEN 'red' WHEN 1 THEN 'green' ELSE 'blue' END,
                'tags', json_array('t' || (i % 5), 't' || (i % 7))),
    date('2020-01-01', '+' || (i % 2000) || ' days'),
    datetime('2026-01-01', '+' || (i * 37) || ' minutes')
FROM n;

CREATE TABLE notes (
    id    INTEGER PRIMARY KEY,
    kind  TEXT NOT NULL,
    body  TEXT
);
INSERT INTO notes (kind, body) VALUES
    ('empty',     ''),
    ('null',      NULL),
    ('multiline', 'first line' || char(10) || 'second line' || char(10) || char(9) || 'indented'),
    ('quotes',    'it''s a "quoted" \back\slash''; DROP TABLE notes; --'),
    ('unicode',   'Привет, мир 👋 — ü ñ 中文 עברית'),
    ('spaces',    '   leading and trailing   '),
    ('long',      (SELECT group_concat('line ' || value, char(10)) FROM generate_series(1, 400))),
    ('huge',      replace(printf('%.70000c', 'x'), 'x', 'abcdefghij'));  -- ~700 KB, over the 64 KB edit limit

-- Text that is not valid UTF-8: shown, but not editable.
INSERT INTO notes (kind, body) VALUES ('invalid utf-8', CAST(x'48656c6c6ffffe20776f726c64' AS TEXT));

CREATE TABLE files (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL,
    content BLOB
);
INSERT INTO files (name, content) VALUES
    ('tiny.bin',  x'00010203'),
    ('png-head',  x'89504e470d0a1a0a0000000d49484452'),
    ('random',    randomblob(4096)),
    ('empty',     x''),
    ('missing',   NULL);

CREATE TABLE wide (
    id INTEGER PRIMARY KEY,
    c01 TEXT, c02 TEXT, c03 TEXT, c04 TEXT, c05 TEXT, c06 TEXT, c07 TEXT, c08 TEXT,
    c09 TEXT, c10 TEXT, c11 TEXT, c12 TEXT, c13 TEXT, c14 TEXT, c15 TEXT, c16 TEXT,
    c17 TEXT, c18 TEXT, c19 TEXT, c20 TEXT, c21 TEXT, c22 TEXT, c23 TEXT, c24 TEXT,
    a_rather_long_column_name_that_should_be_truncated_in_the_header TEXT
);
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 50)
INSERT INTO wide
SELECT i,
    'r' || i || 'c01', 'r' || i || 'c02', 'r' || i || 'c03', 'r' || i || 'c04',
    'r' || i || 'c05', 'r' || i || 'c06', 'r' || i || 'c07', 'r' || i || 'c08',
    'r' || i || 'c09', 'r' || i || 'c10', 'r' || i || 'c11', 'r' || i || 'c12',
    'r' || i || 'c13', 'r' || i || 'c14', 'r' || i || 'c15', 'r' || i || 'c16',
    'r' || i || 'c17', 'r' || i || 'c18', 'r' || i || 'c19', 'r' || i || 'c20',
    'r' || i || 'c21', 'r' || i || 'c22', 'r' || i || 'c23', 'r' || i || 'c24',
    'long value in row ' || i
FROM n;

CREATE TABLE "odd ""name"" ü" (
    "sel""ect `col" TEXT PRIMARY KEY,
    "column with spaces" INTEGER
);
INSERT INTO "odd ""name"" ü" VALUES ('a', 1), ('b', 2);

CREATE TABLE empty_table (
    id   INTEGER PRIMARY KEY,
    note TEXT
);

CREATE INDEX products_title ON products (title);
CREATE TRIGGER products_touch AFTER UPDATE OF price ON products
BEGIN
    UPDATE products SET updated_at = datetime('now') WHERE id = NEW.id;
END;

CREATE VIEW cheap_products AS SELECT id, sku, title, price FROM products WHERE price < 50;
