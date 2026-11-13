use std::{fmt, fs, io::Write, path::PathBuf};

use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};
use serde::{Deserialize, Serialize};
use toml_edit::{ArrayOfTables, DocumentMut, Item};

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

#[derive(Clone, Serialize, Deserialize)]
pub struct Connection {
    pub name: String,
    pub kind: DbKind,
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
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let password = if self.password.is_empty() { "" } else { "***" };
        f.debug_struct("Connection")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("database", &self.database)
            .field("password_storage", &self.password_storage)
            .field("password", &password)
            .finish()
    }
}

impl Connection {
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

const CONNECTIONS_KEY: &str = "connection";
const MANAGED_KEYS: [&str; 8] = [
    "name",
    "kind",
    "host",
    "port",
    "user",
    "database",
    "password_storage",
    "password",
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

    fn write(&self, doc: &DocumentMut) -> Result<()> {
        write_private(&self.path, &doc.to_string())
    }
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
            host: String::new(),
            port: None,
            user: String::new(),
            database: "/tmp/archive.db".into(),
            password_storage: PasswordStorage::Keychain,
            password: String::new(),
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
            host: "localhost".into(),
            port: Some(3306),
            user: "root".into(),
            database: "shop".into(),
            password_storage: PasswordStorage::Keychain,
            password: String::new(),
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
            host: "localhost".into(),
            port: None,
            user: "postgres".into(),
            database: "app".into(),
            password_storage: PasswordStorage::Plain,
            password: "s3cret".into(),
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
        let text = format!("{:?}", plain_dev());
        assert!(!text.contains("s3cret"), "{text}");
        assert!(text.contains("***"), "{text}");
    }
}
