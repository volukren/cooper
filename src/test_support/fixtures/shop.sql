BEGIN;

CREATE TABLE customers (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    email      TEXT    UNIQUE,
    country    TEXT    NOT NULL,
    created_at TEXT    NOT NULL,
    is_active  INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE products (
    id    INTEGER PRIMARY KEY,
    name  TEXT    NOT NULL,
    sku   TEXT    NOT NULL UNIQUE,
    price REAL    NOT NULL,
    stock INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE orders (
    id          INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers(id),
    status      TEXT    NOT NULL DEFAULT 'new',
    ordered_at  TEXT    NOT NULL
);
CREATE INDEX orders_customer ON orders(customer_id);

CREATE TABLE order_items (
    id         INTEGER PRIMARY KEY,
    order_id   INTEGER NOT NULL REFERENCES orders(id),
    product_id INTEGER NOT NULL REFERENCES products(id),
    quantity   INTEGER NOT NULL DEFAULT 1,
    unit_price REAL    NOT NULL
);
CREATE INDEX order_items_order ON order_items(order_id);

CREATE VIEW v_order_totals AS
SELECT o.id                            AS order_id,
       c.name                          AS customer,
       o.status                        AS status,
       sum(i.quantity * i.unit_price)  AS total
FROM orders o
JOIN customers c ON c.id = o.customer_id
JOIN order_items i ON i.order_id = o.id
GROUP BY o.id;

INSERT INTO customers (id, name, email, country, created_at, is_active) VALUES
    (1, 'Alice Johnson',   'alice@example.com',  'US', '2024-01-05 09:12:00', 1),
    (2, 'José García',     'jose@example.com',   'ES', '2024-01-17 14:30:00', 1),
    (3, 'Chen Wei',        'chen@example.com',   'CN', '2024-02-02 08:45:00', 1),
    (4, 'Emma Müller',     'emma@example.com',   'DE', '2024-02-20 19:05:00', 0),
    (5, 'Fatima Al-Sayed', 'fatima@example.com', 'AE', '2024-03-11 11:00:00', 1),
    (6, 'Liam O''Brien',   'liam@example.com',   'IE', '2024-03-29 16:20:00', 1),
    (7, 'Yuki Tanaka',     'yuki@example.com',   'JP', '2024-04-08 07:55:00', 1),
    (8, 'Bob',             NULL,                 'GB', '2024-04-21 21:40:00', 0);

INSERT INTO products (id, name, sku, price, stock) VALUES
    (1, 'Espresso machine', 'ESP-001', 249.00,  12),
    (2, 'Coffee grinder',   'GRD-002',  89.50,  30),
    (3, 'Milk frother',     'FRT-003',  24.90,  75),
    (4, 'Filter papers',    'FLT-004',   4.20, 400),
    (5, 'Ceramic mug',      'MUG-005',  12.00,   0);

INSERT INTO orders (id, customer_id, status, ordered_at) VALUES
    (1, 1, 'shipped',   '2024-05-01 10:00:00'),
    (2, 2, 'shipped',   '2024-05-03 12:30:00'),
    (3, 1, 'new',       '2024-05-09 18:15:00'),
    (4, 5, 'cancelled', '2024-05-12 09:40:00'),
    (5, 7, 'paid',      '2024-05-20 13:05:00'),
    (6, 3, 'paid',      '2024-05-22 15:50:00');

INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES
    (1, 1, 1, 1, 249.00),
    (2, 1, 4, 2,   4.20),
    (3, 2, 2, 1,  89.50),
    (4, 3, 5, 4,  12.00),
    (5, 4, 3, 1,  24.90),
    (6, 5, 1, 1, 249.00),
    (7, 5, 3, 1,  24.90),
    (8, 6, 4, 10,  4.20);

COMMIT;
