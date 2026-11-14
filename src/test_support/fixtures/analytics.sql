BEGIN;

CREATE TABLE users (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    plan       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE events (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id),
    kind        TEXT    NOT NULL,
    occurred_at TEXT    NOT NULL
);
CREATE INDEX events_user ON events(user_id);

WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10)
INSERT INTO users (id, name, plan, created_at)
SELECT i,
       'user ' || i,
       CASE i % 3 WHEN 0 THEN 'team' WHEN 1 THEN 'free' ELSE 'pro' END,
       date('2024-01-01', '+' || (i * 7) || ' days')
FROM n;

WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 3000)
INSERT INTO events (id, user_id, kind, occurred_at)
SELECT i,
       1 + (i * 7) % 10,
       CASE i % 4 WHEN 0 THEN 'login' WHEN 1 THEN 'view' WHEN 2 THEN 'click' ELSE 'purchase' END,
       datetime('2024-06-01 00:00:00', '+' || (i * 13) || ' minutes')
FROM n;

COMMIT;
