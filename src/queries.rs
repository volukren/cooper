use std::path::PathBuf;

use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::connection::{read_optional, write_private};

const QUERIES_KEY: &str = "query";
const CONNECTION: &str = "connection";
const NAME: &str = "name";
const SQL: &str = "sql";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
}

#[derive(Debug, Clone)]
pub struct QueryStore {
    path: PathBuf,
}

impl QueryStore {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self, connection: &str) -> Result<Vec<SavedQuery>> {
        let mut doc = self.read()?;
        let Some(tables) = self.tables(&mut doc)? else {
            return Ok(Vec::new());
        };
        let mut queries: Vec<SavedQuery> = tables
            .iter()
            .filter(|table| field(table, CONNECTION) == Some(connection))
            .filter_map(|table| {
                Some(SavedQuery {
                    name: field(table, NAME)?.to_string(),
                    sql: field(table, SQL)?.to_string(),
                })
            })
            .collect();
        queries.sort_by_cached_key(|query| query.name.to_lowercase());
        Ok(queries)
    }

    pub fn save(&self, connection: &str, query: &SavedQuery) -> Result<()> {
        let mut doc = self.read()?;
        let tables = doc
            .entry(QUERIES_KEY)
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
            .as_array_of_tables_mut()
            .ok_or_else(|| not_a_list(&self.path))?;
        let existing = tables
            .iter_mut()
            .find(|table| is(table, connection, &query.name));
        match existing {
            Some(table) => {
                table.insert(SQL, value(query.sql.as_str()));
            }
            None => {
                let mut table = Table::new();
                table.insert(CONNECTION, value(connection));
                table.insert(NAME, value(query.name.as_str()));
                table.insert(SQL, value(query.sql.as_str()));
                tables.push(table);
            }
        }
        write_private(&self.path, &doc.to_string())
    }

    pub fn remove(&self, connection: &str, name: &str) -> Result<bool> {
        self.retain(|table| !is(table, connection, name))
    }

    pub fn remove_connection(&self, connection: &str) -> Result<bool> {
        self.retain(|table| field(table, CONNECTION) != Some(connection))
    }

    pub fn rename_connection(&self, from: &str, to: &str) -> Result<()> {
        let mut doc = self.read()?;
        let Some(tables) = self.tables(&mut doc)? else {
            return Ok(());
        };
        let mut changed = false;
        for table in tables
            .iter_mut()
            .filter(|table| field(table, CONNECTION) == Some(from))
        {
            table.insert(CONNECTION, value(to));
            changed = true;
        }
        if changed {
            write_private(&self.path, &doc.to_string())?;
        }
        Ok(())
    }

    fn retain(&self, keep: impl Fn(&Table) -> bool) -> Result<bool> {
        let mut doc = self.read()?;
        let Some(tables) = self.tables(&mut doc)? else {
            return Ok(false);
        };
        let before = tables.len();
        tables.retain(|table| keep(table));
        if tables.len() == before {
            return Ok(false);
        }
        write_private(&self.path, &doc.to_string())?;
        Ok(true)
    }

    fn read(&self) -> Result<DocumentMut> {
        match read_optional(&self.path)? {
            Some(data) => data
                .parse()
                .wrap_err_with(|| format!("parsing {}", self.path.display())),
            None => Ok(DocumentMut::new()),
        }
    }

    fn tables<'a>(&self, doc: &'a mut DocumentMut) -> Result<Option<&'a mut ArrayOfTables>> {
        match doc.get_mut(QUERIES_KEY) {
            None => Ok(None),
            Some(item) => item
                .as_array_of_tables_mut()
                .map(Some)
                .ok_or_else(|| not_a_list(&self.path)),
        }
    }
}

fn field<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(Item::as_str)
}

fn is(table: &Table, connection: &str, name: &str) -> bool {
    field(table, CONNECTION) == Some(connection) && field(table, NAME) == Some(name)
}

fn not_a_list(path: &std::path::Path) -> color_eyre::Report {
    eyre!(
        "`{QUERIES_KEY}` in {} is not an array of tables ([[{QUERIES_KEY}]])",
        path.display()
    )
}

#[cfg(test)]
mod tests;
