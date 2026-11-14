use crate::connection::DbKind;

pub fn placeholder(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Postgres => "postgres://user:password@host:5432/database",
        DbKind::MySql => "mysql://user:password@host:3306/database",
        DbKind::Sqlite => "sqlite:///path/to/database.sqlite",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    pub kind: DbKind,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl ParsedUrl {
    fn new(kind: DbKind) -> Self {
        Self {
            kind,
            host: String::new(),
            port: None,
            user: String::new(),
            password: String::new(),
            database: String::new(),
        }
    }
}

pub fn parse(url: &str) -> Result<ParsedUrl, String> {
    let url = url.trim();
    let url = strip_prefix_ignore_case(url, "jdbc:").unwrap_or(url);
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err(format!(
            "expected a URL like {}",
            placeholder(DbKind::Postgres)
        ));
    };
    let kind = match scheme.to_ascii_lowercase().as_str() {
        "postgres" | "postgresql" => DbKind::Postgres,
        "mysql" | "mariadb" => DbKind::MySql,
        "sqlite" | "sqlite3" | "file" => DbKind::Sqlite,
        _ => {
            return Err(format!(
                "unsupported scheme \"{scheme}\", expected postgres://, mysql:// or sqlite://"
            ));
        }
    };
    if kind.is_file_based() {
        return parse_file(rest);
    }
    let Some(rest) = rest.strip_prefix("//") else {
        return Err(format!("expected {scheme}:// at the start"));
    };
    parse_server(kind, rest)
}

fn parse_file(rest: &str) -> Result<ParsedUrl, String> {
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let path = rest.split(['?', '#']).next().unwrap_or_default();
    let database = decode(path)?;
    if database.is_empty() {
        return Err("the database file path is missing".into());
    }
    Ok(ParsedUrl {
        database,
        ..ParsedUrl::new(DbKind::Sqlite)
    })
}

fn parse_server(kind: DbKind, rest: &str) -> Result<ParsedUrl, String> {
    let rest = rest.split('#').next().unwrap_or_default();
    let (main, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = main.split_once('/').unwrap_or((main, ""));
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(userinfo), hostport),
        None => (None, authority),
    };

    let mut parsed = ParsedUrl::new(kind);
    if let Some(userinfo) = userinfo {
        let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
        parsed.user = decode(user)?;
        parsed.password = decode(password)?;
    }
    if hostport.contains(',') {
        return Err("several hosts are not supported".into());
    }
    let (host, port) = split_host_port(hostport)?;
    parsed.host = decode(host)?;
    parsed.port = parse_port(port)?;
    parsed.database = decode(path.trim_end_matches('/'))?;

    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = decode(value)?;
        match key {
            "host" => parsed.host = value,
            "port" => parsed.port = parse_port(&value)?,
            "user" => parsed.user = value,
            "password" => parsed.password = value,
            "dbname" | "database" => parsed.database = value,
            _ => {}
        }
    }
    if parsed.host.is_empty() {
        parsed.host = "localhost".into();
    }
    Ok(parsed)
}

fn split_host_port(hostport: &str) -> Result<(&str, &str), String> {
    if let Some(bracketed) = hostport.strip_prefix('[') {
        let (host, after) = bracketed
            .split_once(']')
            .ok_or("the IPv6 address is missing its closing \"]\"")?;
        let port = match after {
            "" => "",
            _ => after
                .strip_prefix(':')
                .ok_or_else(|| format!("unexpected \"{after}\" after the host"))?,
        };
        return Ok((host, port));
    }
    Ok(hostport.split_once(':').unwrap_or((hostport, "")))
}

fn parse_port(port: &str) -> Result<Option<u16>, String> {
    match port {
        "" => Ok(None),
        p => p
            .parse::<u16>()
            .map(Some)
            .map_err(|_| format!("invalid port \"{p}\"")),
    }
}

fn decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|&b| (b as char).to_digit(16));
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| format!("\"{text}\" is not valid UTF-8 once decoded"))
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(
        kind: DbKind,
        host: &str,
        port: Option<u16>,
        user: &str,
        password: &str,
        database: &str,
    ) -> ParsedUrl {
        ParsedUrl {
            kind,
            host: host.into(),
            port,
            user: user.into(),
            password: password.into(),
            database: database.into(),
        }
    }

    #[test]
    fn a_full_postgres_url() {
        assert_eq!(
            parse("postgres://admin:s3cret@db.example.com:6543/app").unwrap(),
            server(
                DbKind::Postgres,
                "db.example.com",
                Some(6543),
                "admin",
                "s3cret",
                "app"
            )
        );
        assert_eq!(
            parse("  postgresql://admin@db/app?sslmode=require  ").unwrap(),
            server(DbKind::Postgres, "db", None, "admin", "", "app")
        );
    }

    #[test]
    fn mysql_mariadb_and_jdbc_urls() {
        let expected = server(DbKind::MySql, "localhost", Some(3306), "root", "", "shop");
        assert_eq!(parse("mysql://root@localhost:3306/shop").unwrap(), expected);
        assert_eq!(
            parse("mariadb://root@localhost:3306/shop").unwrap(),
            expected
        );
        assert_eq!(
            parse("JDBC:mysql://root@localhost:3306/shop").unwrap(),
            expected
        );
    }

    #[test]
    fn missing_parts_are_left_empty_and_the_host_defaults_to_localhost() {
        assert_eq!(
            parse("postgres://").unwrap(),
            server(DbKind::Postgres, "localhost", None, "", "", "")
        );
        assert_eq!(
            parse("postgres:///app").unwrap(),
            server(DbKind::Postgres, "localhost", None, "", "", "app")
        );
    }

    #[test]
    fn credentials_and_names_are_percent_decoded() {
        let parsed = parse("postgres://us%40er:p%40ss%3Aw%2Frd@db/my%20app").unwrap();
        assert_eq!(parsed.user, "us@er");
        assert_eq!(parsed.password, "p@ss:w/rd");
        assert_eq!(parsed.database, "my app");
    }

    #[test]
    fn the_password_may_contain_unencoded_at_and_colon() {
        let parsed = parse("mysql://root:p@ss:word@db:3307/shop").unwrap();
        assert_eq!(parsed.user, "root");
        assert_eq!(parsed.password, "p@ss:word");
        assert_eq!(parsed.host, "db");
        assert_eq!(parsed.port, Some(3307));
    }

    #[test]
    fn ipv6_hosts() {
        let parsed = parse("postgres://[::1]:5433/app").unwrap();
        assert_eq!((parsed.host.as_str(), parsed.port), ("::1", Some(5433)));
        let parsed = parse("postgres://u@[fe80::1]/app").unwrap();
        assert_eq!((parsed.host.as_str(), parsed.port), ("fe80::1", None));
    }

    #[test]
    fn query_parameters_override_the_url() {
        let parsed =
            parse("postgresql:///app?host=db&port=6000&user=me&password=p%20w&dbname=other")
                .unwrap();
        assert_eq!(
            parsed,
            server(DbKind::Postgres, "db", Some(6000), "me", "p w", "other")
        );
    }

    #[test]
    fn sqlite_urls() {
        for (url, path) in [
            ("sqlite:///data/app.db", "/data/app.db"),
            ("sqlite://app.db", "app.db"),
            ("sqlite:app.db", "app.db"),
            ("sqlite3:///tmp/a%20b.db?mode=ro", "/tmp/a b.db"),
            ("file:///tmp/x.sqlite", "/tmp/x.sqlite"),
        ] {
            let parsed = parse(url).unwrap();
            assert_eq!(parsed.kind, DbKind::Sqlite, "{url}");
            assert_eq!(parsed.database, path, "{url}");
            assert!(parsed.host.is_empty() && parsed.port.is_none(), "{url}");
        }
    }

    #[test]
    fn bad_urls_explain_what_is_wrong() {
        for (url, message) in [
            ("db.example.com", "expected a URL like"),
            ("redis://localhost", "unsupported scheme \"redis\""),
            ("postgres:app", "expected postgres:// at the start"),
            ("postgres://db:port/app", "invalid port \"port\""),
            ("postgres://db:70000/app", "invalid port \"70000\""),
            ("postgres://a:5432,b:5432/app", "several hosts"),
            ("postgres://[::1/app", "closing \"]\""),
            ("postgres://[::1]x/app", "unexpected \"x\""),
            ("sqlite://", "file path is missing"),
            ("postgres://db/%FF", "not valid UTF-8"),
        ] {
            let err = parse(url).unwrap_err();
            assert!(err.contains(message), "{url}: {err}");
        }
    }

    #[test]
    fn every_placeholder_parses_as_its_own_kind() {
        for kind in [DbKind::Postgres, DbKind::MySql, DbKind::Sqlite] {
            assert_eq!(parse(placeholder(kind)).unwrap().kind, kind);
        }
    }

    #[test]
    fn a_stray_percent_sign_is_kept() {
        assert_eq!(parse("postgres://u:100%@db/app").unwrap().password, "100%");
    }
}
