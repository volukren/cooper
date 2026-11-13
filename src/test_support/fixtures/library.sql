-- Sample "library" database for the UI tests.
-- Tests use it as a second connection and do not depend on its rows.
BEGIN;

CREATE TABLE authors (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE books (
    id        INTEGER PRIMARY KEY,
    author_id INTEGER NOT NULL REFERENCES authors(id),
    title     TEXT    NOT NULL,
    year      INTEGER
);

CREATE TABLE members (
    id        INTEGER PRIMARY KEY,
    name      TEXT NOT NULL,
    joined_on TEXT NOT NULL
);

CREATE TABLE loans (
    id          INTEGER PRIMARY KEY,
    book_id     INTEGER NOT NULL REFERENCES books(id),
    member_id   INTEGER NOT NULL REFERENCES members(id),
    loaned_on   TEXT    NOT NULL,
    returned_on TEXT
);

CREATE VIEW v_open_loans AS
SELECT l.id, b.title, m.name AS member, l.loaned_on
FROM loans l
JOIN books b ON b.id = l.book_id
JOIN members m ON m.id = l.member_id
WHERE l.returned_on IS NULL;

INSERT INTO authors (id, name) VALUES
    (1, 'Ursula K. Le Guin'),
    (2, 'Stanisław Lem'),
    (3, 'Octavia E. Butler');

INSERT INTO books (id, author_id, title, year) VALUES
    (1, 1, 'A Wizard of Earthsea',      1968),
    (2, 1, 'The Left Hand of Darkness', 1969),
    (3, 2, 'Solaris',                   1961),
    (4, 2, 'The Cyberiad',              1965),
    (5, 3, 'Kindred',                   1979);

INSERT INTO members (id, name, joined_on) VALUES
    (1, 'Nora Lind',    '2023-09-01'),
    (2, 'Samir Haddad', '2023-11-15'),
    (3, 'Ines Costa',   '2024-02-03');

INSERT INTO loans (id, book_id, member_id, loaned_on, returned_on) VALUES
    (1, 3, 1, '2024-04-02', '2024-04-20'),
    (2, 1, 2, '2024-04-10', NULL),
    (3, 5, 3, '2024-05-01', NULL),
    (4, 2, 1, '2024-05-06', '2024-05-19');

COMMIT;
