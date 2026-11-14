use std::{collections::HashMap, fmt, fs, io::Write, path::PathBuf};

use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};
use serde::{Deserialize, Serialize};
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbKind {
    Postgres,
    MySql,
    Sqlite,
}

impl DbKind {
    const ALL: [DbKind; 3] = [DbKind::Postgres, DbKind::MySql, DbKind::Sqlite];

    pub fn label(self) -> &'static str {
        match self {
            DbKind::Postgres => "PostgreSQL",
            DbKind::MySql => "MySQL",
            DbKind::Sqlite => "SQLite",
        }
    }

    pub fn default_port(self) -> Option<u16> {
        match self {
            DbKind::Postgres => Some(5432),
            DbKind::MySql => Some(3306),
            DbKind::Sqlite => None,
        }
    }

    pub fn is_file_based(self) -> bool {
        matches!(self, DbKind::Sqlite)
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&k| k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|&k| k == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PasswordStorage {
    #[default]
    Keychain,
    Plain,
}

impl PasswordStorage {
    pub fn label(self) -> &'static str {
        match self {
            PasswordStorage::Keychain => "in system keychain",
            PasswordStorage::Plain => "as plain text in config",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            PasswordStorage::Keychain => PasswordStorage::Plain,
            PasswordStorage::Plain => PasswordStorage::Keychain,
        }
    }

    fn is_keychain(&self) -> bool {
        *self == PasswordStorage::Keychain
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tag {
    #[serde(alias = "development")]
    Dev,
    #[serde(alias = "testing")]
    Test,
    #[serde(alias = "stage")]
    Staging,
    #[serde(alias = "production")]
    Prod,
}

impl Tag {
    pub fn label(self) -> &'static str {
        match self {
            Tag::Dev => "Development",
            Tag::Test => "Testing",
            Tag::Staging => "Staging",
            Tag::Prod => "Production",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Tag::Dev => "DEV",
            Tag::Test => "TEST",
            Tag::Staging => "STAGING",
            Tag::Prod => "PROD",
        }
    }

    pub fn asks_to_confirm(self) -> bool {
        self == Tag::Prod
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl SslMode {
    pub const ALL: [SslMode; 5] = [
        SslMode::Disable,
        SslMode::Prefer,
        SslMode::Require,
        SslMode::VerifyCa,
        SslMode::VerifyFull,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SslMode::Disable => "Disable",
            SslMode::Prefer => "Prefer",
            SslMode::Require => "Require",
            SslMode::VerifyCa => "Verify CA",
            SslMode::VerifyFull => "Verify Full",
        }
    }

    pub fn uses_tls(self) -> bool {
        self != SslMode::Disable
    }

    fn is_default(&self) -> bool {
        *self == SslMode::default()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Connection {
    pub name: String,
    pub kind: DbKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<Tag>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(default)]
    pub database: String,
    #[serde(default, skip_serializing_if = "PasswordStorage::is_keychain")]
    pub password_storage: PasswordStorage,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    #[serde(default, skip_serializing_if = "SslMode::is_default")]
    pub ssl_mode: SslMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ssl_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ssl_cert: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ssl_ca: String,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let password = if self.password.is_empty() { "" } else { "***" };
        f.debug_struct("Connection")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("tag", &self.tag)
            .field("read_only", &self.read_only)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("database", &self.database)
            .field("password_storage", &self.password_storage)
            .field("password", &password)
            .field("ssl_mode", &self.ssl_mode)
            .field("ssl_key", &self.ssl_key)
            .field("ssl_cert", &self.ssl_cert)
            .field("ssl_ca", &self.ssl_ca)
            .finish()
    }
}

impl Connection {
    pub fn asks_to_confirm(&self) -> bool {
        self.tag.is_some_and(Tag::asks_to_confirm)
    }

    pub fn uses_keychain(&self) -> bool {
        !self.kind.is_file_based() && self.password_storage == PasswordStorage::Keychain
    }

    pub fn summary(&self) -> String {
        let scheme = match self.kind {
            DbKind::Postgres => "postgres",
            DbKind::MySql => "mysql",
            DbKind::Sqlite => return format!("sqlite://{}", self.database),
        };
        let user = if self.user.is_empty() {
            String::new()
        } else {
            format!("{}@", self.user)
        };
        let port = self.port.map(|p| format!(":{p}")).unwrap_or_default();
        format!("{scheme}://{user}{}{port}/{}", self.host, self.database)
    }
}

#[derive(Deserialize)]
struct ConnectionsFile {
    #[serde(default, rename = "connection")]
    connections: Vec<Connection>,
}

pub type RowKeys = HashMap<(String, String), Vec<String>>;

const CONNECTIONS_KEY: &str = "connection";
const ROW_KEY: &str = "row_key";
const MANAGED_KEYS: [&str; 14] = [
    "name",
    "kind",
    "tag",
    "read_only",
    "host",
    "port",
    "user",
    "database",
    "password_storage",
    "password",
    "ssl_mode",
    "ssl_key",
    "ssl_cert",
    "ssl_ca",
];

#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    legacy_path: Option<PathBuf>,
}

impl Store {
    pub fn at(path: PathBuf) -> Self {
        Self {
            path,
            legacy_path: None,
        }
    }

    pub fn default_location() -> Result<Self> {
        let path = config_home()?.join("cooper").join("connections.toml");
        let legacy_path = dirs::config_dir()
            .map(|dir| dir.join("cooper").join("connections.toml"))
            .filter(|legacy| *legacy != path);
        Ok(Self { path, legacy_path })
    }

    pub fn settings_path(&self) -> PathBuf {
        self.path.with_file_name("settings.toml")
    }

    pub fn load(&self) -> Result<Vec<Connection>> {
        let (data, from_legacy) = match read_optional(&self.path)? {
            Some(data) => (data, None),
            None => match &self.legacy_path {
                Some(legacy) => match read_optional(legacy)? {
                    Some(data) => (data, Some(legacy)),
                    None => return Ok(Vec::new()),
                },
                None => return Ok(Vec::new()),
            },
        };
        let source = from_legacy.unwrap_or(&self.path);
        let parse_err = || format!("parsing {}", source.display());
        let mut doc: DocumentMut = data.parse().wrap_err_with(parse_err)?;
        let file: ConnectionsFile =
            toml_edit::de::from_document(doc.clone()).wrap_err_with(parse_err)?;

        let mut had_plaintext = false;
        let mut connections = file.connections;
        for conn in &mut connections {
            if conn.password_storage == PasswordStorage::Keychain && !conn.password.is_empty() {
                had_plaintext = true;
                set_password(&conn.name, &conn.password).wrap_err_with(|| {
                    format!(
                        "moving password of \"{}\" from {} to the system keychain",
                        conn.name,
                        source.display()
                    )
                })?;
                conn.password.clear();
            }
        }

        if had_plaintext || from_legacy.is_some() {
            if let Some(tables) = doc
                .get_mut(CONNECTIONS_KEY)
                .and_then(Item::as_array_of_tables_mut)
            {
                for (table, conn) in tables.iter_mut().zip(&connections) {
                    if conn.password_storage == PasswordStorage::Keychain {
                        table.remove("password");
                    }
                }
            }
            self.write(&doc)?;
        }
        if let Some(legacy) = from_legacy {
            fs::remove_file(legacy)
                .wrap_err_with(|| format!("removing migrated {}", legacy.display()))?;
            if let Some(dir) = legacy.parent() {
                let _ = fs::remove_dir(dir);
            }
        }
        Ok(connections)
    }

    pub fn append(&self, connection: &Connection) -> Result<()> {
        let mut doc: DocumentMut = match read_optional(&self.path)? {
            Some(data) => data
                .parse()
                .wrap_err_with(|| format!("parsing {}", self.path.display()))?,
            None => DocumentMut::new(),
        };
        let entry = toml_edit::ser::to_document(connection)?.as_table().clone();
        doc.entry(CONNECTIONS_KEY)
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
            .as_array_of_tables_mut()
            .ok_or_else(|| {
                eyre!(
                    "`{CONNECTIONS_KEY}` in {} is not an array of tables ([[{CONNECTIONS_KEY}]])",
                    self.path.display()
                )
            })?
            .push(entry);
        self.write(&doc)
    }

    pub fn remove(&self, name: &str) -> Result<bool> {
        let Some(data) = read_optional(&self.path)? else {
            return Ok(false);
        };
        let mut doc: DocumentMut = data
            .parse()
            .wrap_err_with(|| format!("parsing {}", self.path.display()))?;
        let Some(tables) = doc
            .get_mut(CONNECTIONS_KEY)
            .and_then(Item::as_array_of_tables_mut)
        else {
            return Ok(false);
        };
        let before = tables.len();
        tables.retain(|table| table.get("name").and_then(Item::as_str) != Some(name));
        if tables.len() == before {
            return Ok(false);
        }
        self.write(&doc)?;
        Ok(true)
    }

    pub fn update(&self, original: &str, connection: &Connection) -> Result<()> {
        let data = read_optional(&self.path)?
            .ok_or_else(|| eyre!("{} does not exist", self.path.display()))?;
        let mut doc: DocumentMut = data
            .parse()
            .wrap_err_with(|| format!("parsing {}", self.path.display()))?;
        let table = doc
            .get_mut(CONNECTIONS_KEY)
            .and_then(Item::as_array_of_tables_mut)
            .and_then(|tables| {
                tables
                    .iter_mut()
                    .find(|t| t.get("name").and_then(Item::as_str) == Some(original))
            })
            .ok_or_else(|| {
                eyre!(
                    "connection \"{original}\" not found in {}",
                    self.path.display()
                )
            })?;

        let entry = toml_edit::ser::to_document(connection)?;
        for key in MANAGED_KEYS {
            let Some(value) = entry.get(key).and_then(Item::as_value) else {
                table.remove(key);
                continue;
            };
            match table.get_mut(key).and_then(Item::as_value_mut) {
                Some(existing) => {
                    let decor = existing.decor().clone();
                    *existing = value.clone();
                    *existing.decor_mut() = decor;
                }
                None => {
                    table.insert(key, Item::Value(value.clone()));
                }
            }
        }
        self.write(&doc)
    }

    pub fn row_keys(&self) -> HashMap<String, RowKeys> {
        let doc = read_optional(&self.path)
            .ok()
            .flatten()
            .and_then(|data| data.parse::<DocumentMut>().ok());
        let Some(tables) = doc
            .as_ref()
            .and_then(|doc| doc.get(CONNECTIONS_KEY))
            .and_then(Item::as_array_of_tables)
        else {
            return HashMap::new();
        };
        tables
            .iter()
            .filter_map(|table| {
                let name = table.get("name").and_then(Item::as_str)?;
                let keys: RowKeys = table
                    .get(ROW_KEY)
                    .and_then(Item::as_array_of_tables)?
                    .iter()
                    .filter_map(row_key)
                    .collect();
                (!keys.is_empty()).then(|| (name.to_string(), keys))
            })
            .collect()
    }

    pub fn set_row_key(
        &self,
        connection: &str,
        schema: &str,
        table: &str,
        columns: Option<&[String]>,
    ) -> Result<()> {
        let data = read_optional(&self.path)?
            .ok_or_else(|| eyre!("{} does not exist", self.path.display()))?;
        let mut doc: DocumentMut = data
            .parse()
            .wrap_err_with(|| format!("parsing {}", self.path.display()))?;
        let entry = doc
            .get_mut(CONNECTIONS_KEY)
            .and_then(Item::as_array_of_tables_mut)
            .and_then(|tables| {
                tables
                    .iter_mut()
                    .find(|t| t.get("name").and_then(Item::as_str) == Some(connection))
            })
            .ok_or_else(|| {
                eyre!(
                    "connection \"{connection}\" not found in {}",
                    self.path.display()
                )
            })?;
        let keys = entry
            .entry(ROW_KEY)
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
            .as_array_of_tables_mut()
            .ok_or_else(|| {
                eyre!(
                    "`{ROW_KEY}` of \"{connection}\" in {} is not an array of tables \
                     ([[{CONNECTIONS_KEY}.{ROW_KEY}]])",
                    self.path.display()
                )
            })?;
        keys.retain(|key| {
            key.get("schema").and_then(Item::as_str) != Some(schema)
                || key.get("table").and_then(Item::as_str) != Some(table)
        });
        if let Some(columns) = columns {
            let mut key = Table::new();
            key.insert("schema", toml_edit::value(schema));
            key.insert("table", toml_edit::value(table));
            key.insert(
                "columns",
                toml_edit::value(columns.iter().map(String::as_str).collect::<Array>()),
            );
            keys.push(key);
        }
        if keys.is_empty() {
            entry.remove(ROW_KEY);
        }
        self.write(&doc)
    }

    fn write(&self, doc: &DocumentMut) -> Result<()> {
        write_private(&self.path, &doc.to_string())
    }
}

fn row_key(key: &Table) -> Option<((String, String), Vec<String>)> {
    let text = |name: &str| key.get(name).and_then(Item::as_str).map(str::to_string);
    let columns = key
        .get("columns")?
        .as_array()?
        .iter()
        .map(|column| column.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    (!columns.is_empty()).then_some(((text("schema")?, text("table")?), columns))
}

const KEYCHAIN_SERVICE: &str = "cooper";

pub fn set_password(name: &str, password: &str) -> Result<()> {
    keyring::Entry::new(KEYCHAIN_SERVICE, name)?.set_password(password)?;
    Ok(())
}

pub fn password(conn: &Connection) -> Result<Option<String>> {
    if conn.kind.is_file_based() {
        return Ok(None);
    }
    match conn.password_storage {
        PasswordStorage::Plain => Ok(Some(conn.password.clone()).filter(|p| !p.is_empty())),
        PasswordStorage::Keychain => keychain_password(&conn.name),
    }
}

fn keychain_password(name: &str) -> Result<Option<String>> {
    match keyring::Entry::new(KEYCHAIN_SERVICE, name)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn delete_password(name: &str) -> Result<()> {
    match keyring::Entry::new(KEYCHAIN_SERVICE, name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

fn config_home() -> Result<PathBuf> {
    #[cfg(unix)]
    {
        if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
        {
            return Ok(dir);
        }
        dirs::home_dir()
            .map(|home| home.join(".config"))
            .ok_or_else(|| eyre!("could not determine home directory"))
    }
    #[cfg(not(unix))]
    {
        dirs::config_dir().ok_or_else(|| eyre!("could not determine config directory"))
    }
}

pub(crate) fn read_optional(path: &std::path::Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(data) => Ok(Some(data)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).wrap_err_with(|| format!("reading {}", path.display())),
    }
}

pub(crate) fn write_private(path: &std::path::Path, data: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).wrap_err_with(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    let _ = fs::remove_file(&tmp);
    let mut file = open_private(&tmp).wrap_err_with(|| format!("writing {}", tmp.display()))?;
    file.write_all(data.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, path).wrap_err_with(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn open_private(path: &std::path::Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempStore {
        dir: PathBuf,
        store: Store,
    }

    impl TempStore {
        fn new(contents: Option<&str>) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cooper-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("connections.toml");
            if let Some(contents) = contents {
                fs::write(&path, contents).unwrap();
            }
            let store = Store {
                path,
                legacy_path: None,
            };
            Self { dir, store }
        }

        fn contents(&self) -> String {
            fs::read_to_string(&self.store.path).unwrap()
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    const TWO: &str = r#"# my connections

[[connection]]
name = "local"
kind = "sqlite"
database = "/tmp/a.db"

# production, careful
[[connection]]
name = "prod"
kind = "postgres"
host = "db.example.com"
port = 5432
user = "admin"
database = "app"
"#;

    #[test]
    fn remove_deletes_only_the_named_connection() {
        let t = TempStore::new(Some(TWO));
        assert!(t.store.remove("local").unwrap());

        let names: Vec<_> = t
            .store
            .load()
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["prod"]);
        assert!(t.contents().contains("# production, careful"));
    }

    #[test]
    fn remove_keeps_comments_of_other_entries() {
        let t = TempStore::new(Some(TWO));
        assert!(t.store.remove("prod").unwrap());
        let contents = t.contents();
        assert!(contents.contains("# my connections"));
        assert!(!contents.contains("# production, careful"));
        assert!(!contents.contains("db.example.com"));
    }

    #[test]
    fn remove_unknown_name_leaves_file_untouched() {
        let t = TempStore::new(Some(TWO));
        assert!(!t.store.remove("nope").unwrap());
        assert_eq!(t.contents(), TWO);
    }

    #[test]
    fn remove_without_file_is_noop() {
        let t = TempStore::new(None);
        assert!(!t.store.remove("local").unwrap());
        assert!(!t.store.path.exists());
    }

    #[test]
    fn remove_last_connection_leaves_empty_list() {
        let t = TempStore::new(Some(TWO));
        assert!(t.store.remove("local").unwrap());
        assert!(t.store.remove("prod").unwrap());
        assert!(t.store.load().unwrap().is_empty());
    }

    #[test]
    fn update_replaces_the_entry_in_place_and_keeps_unmanaged_content() {
        let t = TempStore::new(Some(
            "# my databases\n\
             [[connection]]\n\
             name = \"prod\"\n\
             kind = \"postgres\"\n\
             host = \"db.example.com\"\n\
             port = 5432\n\
             user = \"admin\"\n\
             database = \"app\"\n\
             color = \"red\"\n\
             \n\
             [[connection]]\n\
             name = \"local\"\n\
             kind = \"sqlite\"\n\
             database = \"/tmp/a.db\"\n",
        ));
        let edited = Connection {
            name: "archive".into(),
            kind: DbKind::Sqlite,
            tag: None,
            read_only: false,
            host: String::new(),
            port: None,
            user: String::new(),
            database: "/tmp/archive.db".into(),
            password_storage: PasswordStorage::Keychain,
            password: String::new(),
            ssl_mode: SslMode::default(),
            ssl_key: String::new(),
            ssl_cert: String::new(),
            ssl_ca: String::new(),
        };
        t.store.update("prod", &edited).unwrap();

        let data = t.contents();
        assert!(data.starts_with("# my databases\n"), "{data}");
        assert!(data.contains("color = \"red\""), "{data}");
        assert!(!data.contains("host"), "{data}");
        assert!(!data.contains("port"), "{data}");
        assert!(!data.contains("user"), "{data}");

        let names: Vec<_> = t
            .store
            .load()
            .unwrap()
            .into_iter()
            .map(|c| (c.name, c.kind, c.database))
            .collect();
        assert_eq!(
            names,
            [
                ("archive".into(), DbKind::Sqlite, "/tmp/archive.db".into()),
                ("local".into(), DbKind::Sqlite, "/tmp/a.db".into()),
            ]
        );
    }

    #[test]
    fn update_adds_fields_the_entry_did_not_have() {
        let t = TempStore::new(Some(
            "[[connection]]\nname = \"x\"\nkind = \"sqlite\"\ndatabase = \"x.db\"\n",
        ));
        let edited = Connection {
            name: "x".into(),
            kind: DbKind::MySql,
            tag: None,
            read_only: false,
            host: "localhost".into(),
            port: Some(3306),
            user: "root".into(),
            database: "shop".into(),
            password_storage: PasswordStorage::Keychain,
            password: String::new(),
            ssl_mode: SslMode::default(),
            ssl_key: String::new(),
            ssl_cert: String::new(),
            ssl_ca: String::new(),
        };
        t.store.update("x", &edited).unwrap();
        let loaded = t.store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].summary(), "mysql://root@localhost:3306/shop");
    }

    #[test]
    fn update_fails_for_an_unknown_connection() {
        let t = TempStore::new(Some(TWO));
        let local = t.store.load().unwrap().remove(0);
        let err = t.store.update("missing", &local).unwrap_err();
        assert!(err.to_string().contains("\"missing\" not found"), "{err}");
        assert_eq!(t.contents(), TWO);
    }

    const PLAIN: &str = r#"[[connection]]
name = "dev"
kind = "postgres"
host = "localhost"
user = "postgres"
database = "app"
password_storage = "plain" # local only
password = "s3cret"
"#;

    fn plain_dev() -> Connection {
        Connection {
            name: "dev".into(),
            kind: DbKind::Postgres,
            tag: None,
            read_only: false,
            host: "localhost".into(),
            port: None,
            user: "postgres".into(),
            database: "app".into(),
            password_storage: PasswordStorage::Plain,
            password: "s3cret".into(),
            ssl_mode: SslMode::default(),
            ssl_key: String::new(),
            ssl_cert: String::new(),
            ssl_ca: String::new(),
        }
    }

    #[test]
    fn plain_text_password_stays_in_the_file() {
        let t = TempStore::new(Some(PLAIN));
        let loaded = t.store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].password_storage, PasswordStorage::Plain);
        assert_eq!(password(&loaded[0]).unwrap().as_deref(), Some("s3cret"));
        assert_eq!(t.contents(), PLAIN);
    }

    #[test]
    fn empty_plain_text_password_means_no_password() {
        let mut conn = plain_dev();
        conn.password.clear();
        assert_eq!(password(&conn).unwrap(), None);
    }

    #[test]
    fn append_writes_a_plain_text_password() {
        let t = TempStore::new(None);
        t.store.append(&plain_dev()).unwrap();
        let data = t.contents();
        assert!(data.contains("password_storage = \"plain\""), "{data}");
        assert!(data.contains("password = \"s3cret\""), "{data}");
        let loaded = t.store.load().unwrap();
        assert_eq!(loaded[0].password, "s3cret");
    }

    #[test]
    fn keychain_connections_do_not_write_storage_or_password_keys() {
        let t = TempStore::new(None);
        let mut conn = plain_dev();
        conn.password_storage = PasswordStorage::Keychain;
        conn.password.clear();
        t.store.append(&conn).unwrap();
        let data = t.contents();
        assert!(!data.contains("password"), "{data}");
    }

    #[test]
    fn switching_to_keychain_removes_the_password_from_the_file() {
        let t = TempStore::new(Some(PLAIN));
        let mut conn = plain_dev();
        conn.password_storage = PasswordStorage::Keychain;
        conn.password.clear();
        t.store.update("dev", &conn).unwrap();
        let data = t.contents();
        assert!(!data.contains("password"), "{data}");
        assert!(!data.contains("s3cret"), "{data}");
    }

    #[test]
    fn switching_to_plain_text_adds_the_password_to_the_file() {
        let t = TempStore::new(Some(
            "[[connection]]\nname = \"dev\"\nkind = \"postgres\"\nhost = \"localhost\"\n\
             user = \"postgres\"\ndatabase = \"app\"\n",
        ));
        t.store.update("dev", &plain_dev()).unwrap();
        let loaded = t.store.load().unwrap();
        assert_eq!(loaded[0].password_storage, PasswordStorage::Plain);
        assert_eq!(loaded[0].password, "s3cret");
    }

    #[test]
    fn changing_a_plain_text_password_keeps_its_comment() {
        let t = TempStore::new(Some(PLAIN));
        let mut conn = plain_dev();
        conn.password = "n3w".into();
        t.store.update("dev", &conn).unwrap();
        let data = t.contents();
        assert!(
            data.contains("password_storage = \"plain\" # local only"),
            "{data}"
        );
        assert!(data.contains("password = \"n3w\""), "{data}");
    }

    #[test]
    fn debug_output_hides_the_password() {
        let conn = Connection {
            tag: Some(Tag::Prod),
            ..plain_dev()
        };
        let text = format!("{conn:?}");
        assert!(!text.contains("s3cret"), "{text}");
        assert!(text.contains("***"), "{text}");
        assert!(text.contains("Prod"), "{text}");
    }

    #[test]
    fn a_tag_is_written_only_when_set() {
        let t = TempStore::new(None);
        let prod = Connection {
            tag: Some(Tag::Prod),
            ..plain_dev()
        };
        t.store.append(&prod).unwrap();
        let data = t.contents();
        assert!(data.contains("tag = \"prod\""), "{data}");

        let t = TempStore::new(None);
        t.store.append(&plain_dev()).unwrap();
        let data = t.contents();
        assert!(!data.contains("tag"), "{data}");
    }

    #[test]
    fn configs_without_tags_load_untagged() {
        let t = TempStore::new(Some(TWO));
        let loaded = t.store.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().all(|c| c.tag.is_none()), "{loaded:?}");
        assert!(!loaded.iter().any(Connection::asks_to_confirm));
    }

    #[test]
    fn update_adds_changes_and_removes_the_tag_in_place() {
        let t = TempStore::new(Some(
            "# my databases\n\
             [[connection]]\n\
             name = \"dev\"\n\
             kind = \"postgres\"\n\
             host = \"localhost\"\n\
             user = \"postgres\"\n\
             database = \"app\" # main schema\n\
             password_storage = \"plain\"\n\
             password = \"s3cret\"\n\
             color = \"red\"\n",
        ));
        let mut conn = Connection {
            tag: Some(Tag::Prod),
            ..plain_dev()
        };
        t.store.update("dev", &conn).unwrap();
        let data = t.contents();
        assert!(data.contains("tag = \"prod\""), "{data}");
        assert!(data.contains("color = \"red\""), "{data}");
        assert!(data.contains("database = \"app\" # main schema"), "{data}");
        assert!(data.starts_with("# my databases\n"), "{data}");
        assert!(t.store.load().unwrap()[0].asks_to_confirm());

        conn.tag = Some(Tag::Dev);
        t.store.update("dev", &conn).unwrap();
        let data = t.contents();
        assert!(data.contains("tag = \"dev\""), "{data}");
        assert!(!data.contains("prod"), "{data}");
        assert!(data.contains("color = \"red\""), "{data}");
        assert_eq!(t.store.load().unwrap()[0].tag, Some(Tag::Dev));

        conn.tag = None;
        t.store.update("dev", &conn).unwrap();
        let data = t.contents();
        assert!(!data.contains("tag"), "{data}");
        assert!(data.contains("color = \"red\""), "{data}");
        assert!(data.contains("database = \"app\" # main schema"), "{data}");
        assert_eq!(t.store.load().unwrap()[0].tag, None);
    }

    #[test]
    fn long_tag_names_are_accepted_and_unknown_ones_fail() {
        let t = TempStore::new(Some(
            "[[connection]]\nname = \"x\"\nkind = \"sqlite\"\n\
             database = \"x.db\"\ntag = \"production\"\n",
        ));
        assert_eq!(t.store.load().unwrap()[0].tag, Some(Tag::Prod));

        let t = TempStore::new(Some(
            "[[connection]]\nname = \"x\"\nkind = \"sqlite\"\n\
             database = \"x.db\"\ntag = \"qa\"\n",
        ));
        let err = t.store.load().unwrap_err();
        assert!(format!("{err:?}").contains("qa"), "{err:?}");
    }

    #[test]
    fn read_only_is_written_only_when_set_and_round_trips() {
        let t = TempStore::new(None);
        t.store.append(&plain_dev()).unwrap();
        let data = t.contents();
        assert!(!data.contains("read_only"), "{data}");
        assert!(!t.store.load().unwrap()[0].read_only);

        let t = TempStore::new(None);
        let conn = Connection {
            read_only: true,
            ..plain_dev()
        };
        t.store.append(&conn).unwrap();
        let data = t.contents();
        assert!(data.contains("read_only = true"), "{data}");
        assert!(t.store.load().unwrap()[0].read_only);
    }

    #[test]
    fn configs_without_read_only_load_writable() {
        let t = TempStore::new(Some(TWO));
        let loaded = t.store.load().unwrap();
        assert!(loaded.iter().all(|c| !c.read_only), "{loaded:?}");
    }

    #[test]
    fn ssl_settings_are_written_only_when_set_and_round_trip() {
        let t = TempStore::new(None);
        t.store.append(&plain_dev()).unwrap();
        let data = t.contents();
        assert!(!data.contains("ssl"), "{data}");
        assert_eq!(t.store.load().unwrap()[0].ssl_mode, SslMode::Prefer);

        for (mode, text) in [
            (SslMode::Disable, "disable"),
            (SslMode::Require, "require"),
            (SslMode::VerifyCa, "verify-ca"),
            (SslMode::VerifyFull, "verify-full"),
        ] {
            let t = TempStore::new(None);
            let conn = Connection {
                ssl_mode: mode,
                ssl_key: "~/certs/client.key".into(),
                ssl_cert: "~/certs/client.pem".into(),
                ssl_ca: "/etc/ssl/ca.pem".into(),
                ..plain_dev()
            };
            t.store.append(&conn).unwrap();
            let data = t.contents();
            assert!(data.contains(&format!("ssl_mode = \"{text}\"")), "{data}");
            assert!(data.contains("ssl_key = \"~/certs/client.key\""), "{data}");
            let loaded = t.store.load().unwrap().remove(0);
            assert_eq!(loaded.ssl_mode, mode);
            assert_eq!(loaded.ssl_key, "~/certs/client.key");
            assert_eq!(loaded.ssl_cert, "~/certs/client.pem");
            assert_eq!(loaded.ssl_ca, "/etc/ssl/ca.pem");
        }
    }

    #[test]
    fn configs_without_ssl_settings_load_with_prefer() {
        let t = TempStore::new(Some(TWO));
        let loaded = t.store.load().unwrap();
        assert!(
            loaded.iter().all(|c| c.ssl_mode == SslMode::Prefer
                && c.ssl_key.is_empty()
                && c.ssl_cert.is_empty()
                && c.ssl_ca.is_empty()),
            "{loaded:?}"
        );
        assert_eq!(t.contents(), TWO);
    }

    #[test]
    fn update_adds_and_removes_read_only_in_place() {
        let t = TempStore::new(Some(PLAIN));
        let mut conn = Connection {
            read_only: true,
            ..plain_dev()
        };
        t.store.update("dev", &conn).unwrap();
        let data = t.contents();
        assert!(data.contains("read_only = true"), "{data}");
        assert!(
            data.contains("password_storage = \"plain\" # local only"),
            "{data}"
        );
        assert!(t.store.load().unwrap()[0].read_only);

        conn.read_only = false;
        t.store.update("dev", &conn).unwrap();
        assert_eq!(t.contents(), PLAIN);
    }

    #[test]
    fn a_hand_written_read_only_false_loads_writable_and_other_values_fail() {
        let entry = "[[connection]]\nname = \"x\"\nkind = \"sqlite\"\ndatabase = \"x.db\"\n";
        let t = TempStore::new(Some(&format!("{entry}read_only = false\n")));
        assert!(!t.store.load().unwrap()[0].read_only);

        let t = TempStore::new(Some(&format!("{entry}read_only = \"yes\"\n")));
        let err = t.store.load().unwrap_err();
        assert!(format!("{err:?}").contains("yes"), "{err:?}");
    }

    #[test]
    fn debug_output_shows_read_only() {
        let conn = Connection {
            read_only: true,
            ..plain_dev()
        };
        let text = format!("{conn:?}");
        assert!(text.contains("read_only: true"), "{text}");
    }

    #[test]
    fn update_adds_and_removes_ssl_settings_in_place() {
        let t = TempStore::new(Some(PLAIN));
        let mut conn = Connection {
            ssl_mode: SslMode::VerifyFull,
            ssl_ca: "ca.pem".into(),
            ..plain_dev()
        };
        t.store.update("dev", &conn).unwrap();
        let data = t.contents();
        assert!(data.contains("ssl_mode = \"verify-full\""), "{data}");
        assert!(data.contains("ssl_ca = \"ca.pem\""), "{data}");
        assert!(!data.contains("ssl_key"), "{data}");

        conn.ssl_mode = SslMode::Prefer;
        conn.ssl_ca.clear();
        t.store.update("dev", &conn).unwrap();
        assert_eq!(t.contents(), PLAIN);
    }

    #[test]
    fn unknown_ssl_modes_fail_to_load() {
        let t = TempStore::new(Some(
            "[[connection]]\nname = \"x\"\nkind = \"postgres\"\n\
             database = \"x\"\nssl_mode = \"sometimes\"\n",
        ));
        let err = t.store.load().unwrap_err();
        assert!(format!("{err:?}").contains("sometimes"), "{err:?}");
    }

    #[test]
    fn debug_output_shows_the_ssl_settings() {
        let conn = Connection {
            ssl_mode: SslMode::VerifyCa,
            ssl_ca: "/etc/ca.pem".into(),
            ..plain_dev()
        };
        let text = format!("{conn:?}");
        assert!(text.contains("VerifyCa"), "{text}");
        assert!(text.contains("/etc/ca.pem"), "{text}");
    }

    #[test]
    fn a_leading_tilde_expands_to_the_home_directory() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~/certs/ca.pem"), home.join("certs/ca.pem"));
        assert_eq!(expand_home("/etc/ca.pem"), PathBuf::from("/etc/ca.pem"));
        assert_eq!(expand_home("~user/ca.pem"), PathBuf::from("~user/ca.pem"));
    }

    #[test]
    fn ssl_mode_labels_are_distinct() {
        let labels: std::collections::HashSet<_> = SslMode::ALL.map(SslMode::label).into();
        assert_eq!(labels.len(), SslMode::ALL.len());
        assert!(!SslMode::Disable.uses_tls());
        assert!(SslMode::ALL[1..].iter().all(|mode| mode.uses_tls()));
    }

    fn columns(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn place(schema: &str, table: &str) -> (String, String) {
        (schema.to_string(), table.to_string())
    }

    fn names(store: &Store) -> Vec<String> {
        store.load().unwrap().into_iter().map(|c| c.name).collect()
    }

    #[test]
    fn row_keys_round_trip_inside_their_connection() {
        let t = TempStore::new(Some(TWO));
        assert!(t.store.row_keys().is_empty());
        let log = columns(&["at", "msg"]);
        t.store
            .set_row_key("local", "main", "log", Some(&log))
            .unwrap();
        t.store
            .set_row_key("local", "main", "pairs", Some(&columns(&["a"])))
            .unwrap();
        t.store
            .set_row_key("prod", "public", "log", Some(&columns(&["id"])))
            .unwrap();
        t.store
            .set_row_key("local", "main", "log", Some(&columns(&["msg"])))
            .unwrap();

        let keys = t.store.row_keys();
        assert_eq!(keys.len(), 2);
        assert_eq!(
            keys["local"],
            RowKeys::from([
                (place("main", "log"), columns(&["msg"])),
                (place("main", "pairs"), columns(&["a"])),
            ])
        );
        assert_eq!(
            keys["prod"],
            RowKeys::from([(place("public", "log"), columns(&["id"]))])
        );
        let data = t.contents();
        assert!(data.contains("[[connection.row_key]]"), "{data}");
        assert!(data.contains("# production, careful"), "{data}");
        assert_eq!(names(&t.store), ["local", "prod"], "load ignores row keys");
    }

    #[test]
    fn a_row_key_of_the_first_connection_stays_with_it() {
        let t = TempStore::new(Some(TWO));
        t.store
            .set_row_key("local", "main", "log", Some(&columns(&["at"])))
            .unwrap();
        let data = t.contents();
        let key = data.find("[[connection.row_key]]").unwrap();
        let prod = data.find("name = \"prod\"").unwrap();
        assert!(key < prod, "written under local, not after prod:\n{data}");

        let doc: DocumentMut = data.parse().unwrap();
        let tables = doc[CONNECTIONS_KEY].as_array_of_tables().unwrap();
        assert!(tables.get(0).unwrap().contains_key(ROW_KEY), "{data}");
        assert!(!tables.get(1).unwrap().contains_key(ROW_KEY), "{data}");
        assert_eq!(
            t.store.row_keys().keys().collect::<Vec<_>>(),
            ["local"],
            "{data}"
        );
    }

    #[test]
    fn row_keys_follow_a_rename_and_go_with_a_removal() {
        let t = TempStore::new(Some(TWO));
        t.store
            .set_row_key("prod", "public", "log", Some(&columns(&["at"])))
            .unwrap();
        t.store
            .set_row_key("local", "main", "log", Some(&columns(&["id"])))
            .unwrap();
        let mut prod = t.store.load().unwrap().remove(1);
        prod.name = "orders".into();
        prod.host = "db2.example.com".into();
        t.store.update("prod", &prod).unwrap();
        let keys = t.store.row_keys();
        assert!(!keys.contains_key("prod"));
        assert_eq!(
            keys["orders"],
            RowKeys::from([(place("public", "log"), columns(&["at"]))])
        );
        assert_eq!(names(&t.store), ["local", "orders"]);

        assert!(t.store.remove("orders").unwrap());
        let keys = t.store.row_keys();
        assert_eq!(keys.keys().collect::<Vec<_>>(), ["local"]);
        assert!(!t.contents().contains("public"), "{}", t.contents());
    }

    #[test]
    fn forgetting_a_row_key_removes_the_entry_and_then_the_list() {
        let t = TempStore::new(Some(TWO));
        for table in ["log", "pairs"] {
            t.store
                .set_row_key("local", "main", table, Some(&columns(&["a"])))
                .unwrap();
        }
        t.store.set_row_key("local", "main", "log", None).unwrap();
        assert_eq!(
            t.store.row_keys()["local"],
            RowKeys::from([(place("main", "pairs"), columns(&["a"]))])
        );
        t.store.set_row_key("local", "main", "pairs", None).unwrap();
        assert!(t.store.row_keys().is_empty());
        assert!(!t.contents().contains(ROW_KEY), "{}", t.contents());
        assert_eq!(names(&t.store), ["local", "prod"]);
    }

    #[test]
    fn a_row_key_needs_a_known_connection() {
        let t = TempStore::new(Some(TWO));
        let err = t
            .store
            .set_row_key("missing", "main", "log", Some(&columns(&["a"])))
            .unwrap_err();
        assert!(err.to_string().contains("\"missing\" not found"), "{err}");
        assert_eq!(t.contents(), TWO);
    }

    #[test]
    fn without_a_file_there_are_no_row_keys() {
        let t = TempStore::new(None);
        assert!(t.store.row_keys().is_empty());
        assert!(t.store.set_row_key("local", "main", "log", None).is_err());
        assert!(!t.store.path.exists());
    }

    #[test]
    fn malformed_row_keys_are_skipped() {
        let t = TempStore::new(Some(
            r#"[[connection]]
name = "local"
kind = "sqlite"
database = "/tmp/a.db"

[[connection.row_key]]
schema = "main"
table = "text"
columns = "a"

[[connection.row_key]]
schema = "main"
table = "empty"
columns = []

[[connection.row_key]]
schema = "main"
columns = ["a"]

[[connection.row_key]]
schema = "main"
table = "numbers"
columns = [1]

[[connection.row_key]]
schema = "main"
table = "good"
columns = ["a", "b"]

[[connection]]
name = "prod"
kind = "postgres"
database = "app"
row_key = "nonsense"
"#,
        ));
        assert_eq!(names(&t.store), ["local", "prod"]);
        let keys = t.store.row_keys();
        assert_eq!(keys.keys().collect::<Vec<_>>(), ["local"]);
        assert_eq!(
            keys["local"],
            RowKeys::from([(place("main", "good"), columns(&["a", "b"]))])
        );
        assert!(
            t.store
                .set_row_key("prod", "public", "log", Some(&columns(&["a"])))
                .is_err(),
            "a row_key that is not a list of tables is left alone"
        );
    }

    #[test]
    fn a_broken_file_has_no_row_keys() {
        let t = TempStore::new(Some("[[connection]\nname = "));
        assert!(t.store.row_keys().is_empty());
    }
}
