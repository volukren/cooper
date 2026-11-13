use std::fs;

use super::*;

fn store(dir: &tempfile::TempDir) -> QueryStore {
    QueryStore::at(dir.path().join("queries.toml"))
}

fn query(name: &str, sql: &str) -> SavedQuery {
    SavedQuery {
        name: name.to_string(),
        sql: sql.to_string(),
    }
}

fn names(store: &QueryStore, connection: &str) -> Vec<String> {
    store
        .load(connection)
        .unwrap()
        .into_iter()
        .map(|query| query.name)
        .collect()
}

#[test]
fn missing_file_has_no_queries_and_removing_does_not_create_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    assert!(store.load("prod").unwrap().is_empty());
    assert!(!store.remove("prod", "x").unwrap());
    assert!(!store.remove_connection("prod").unwrap());
    store.rename_connection("prod", "live").unwrap();
    assert!(!dir.path().join("queries.toml").exists());
}

#[test]
fn saved_queries_round_trip_and_stay_readable_in_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let sql =
        "SELECT id, \"name\"\nFROM users\nWHERE note = 'it''s \\ here'\n  AND x = \"\"\"q\"\"\";";
    store.save("prod", &query("active users", sql)).unwrap();
    store.save("prod", &query("one-liner", "SELECT 1")).unwrap();

    assert_eq!(
        store.load("prod").unwrap(),
        [query("active users", sql), query("one-liner", "SELECT 1")]
    );
    let data = fs::read_to_string(dir.path().join("queries.toml")).unwrap();
    assert!(data.contains("[[query]]\nconnection = \"prod\""), "{data}");
    assert!(data.contains("\nFROM users\n"), "{data}");
}

#[test]
fn queries_belong_to_their_connection_and_sort_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    store.save("prod", &query("zeta", "SELECT 1")).unwrap();
    store.save("local", &query("mine", "SELECT 2")).unwrap();
    store.save("prod", &query("Alpha", "SELECT 3")).unwrap();
    store.save("prod", &query("beta", "SELECT 4")).unwrap();

    assert_eq!(names(&store, "prod"), ["Alpha", "beta", "zeta"]);
    assert_eq!(names(&store, "local"), ["mine"]);
    assert!(names(&store, "other").is_empty());
}

#[test]
fn saving_an_existing_name_replaces_it_and_keeps_the_rest_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queries.toml");
    fs::write(
        &path,
        "# my queries\n\
         [[query]]\n\
         connection = \"prod\"\n\
         name = \"count\"\n\
         sql = \"SELECT 1\"\n\
         tag = \"keep me\"\n\
         \n\
         # same name, another connection\n\
         [[query]]\n\
         connection = \"local\"\n\
         name = \"count\"\n\
         sql = \"SELECT 2\"\n",
    )
    .unwrap();
    let store = QueryStore::at(path.clone());
    store
        .save("prod", &query("count", "SELECT count(*)\nFROM users"))
        .unwrap();

    assert_eq!(
        store.load("prod").unwrap(),
        [query("count", "SELECT count(*)\nFROM users")]
    );
    assert_eq!(store.load("local").unwrap(), [query("count", "SELECT 2")]);
    let data = fs::read_to_string(&path).unwrap();
    assert!(data.starts_with("# my queries\n"), "{data}");
    assert!(data.contains("tag = \"keep me\""), "{data}");
    assert!(data.contains("# same name, another connection"), "{data}");
}

#[test]
fn remove_deletes_only_the_named_query() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    store.save("prod", &query("a", "SELECT 1")).unwrap();
    store.save("prod", &query("b", "SELECT 2")).unwrap();
    store.save("local", &query("a", "SELECT 3")).unwrap();

    assert!(store.remove("prod", "a").unwrap());
    assert!(!store.remove("prod", "a").unwrap());
    assert_eq!(names(&store, "prod"), ["b"]);
    assert_eq!(names(&store, "local"), ["a"]);
}

#[test]
fn queries_follow_a_renamed_connection_and_go_with_a_deleted_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    store.save("prod", &query("a", "SELECT 1")).unwrap();
    store.save("prod", &query("b", "SELECT 2")).unwrap();
    store.save("local", &query("c", "SELECT 3")).unwrap();

    store.rename_connection("prod", "live").unwrap();
    assert!(names(&store, "prod").is_empty());
    assert_eq!(names(&store, "live"), ["a", "b"]);

    assert!(store.remove_connection("live").unwrap());
    assert!(names(&store, "live").is_empty());
    assert_eq!(names(&store, "local"), ["c"]);
}

#[test]
fn incomplete_entries_are_skipped_and_a_wrong_shape_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queries.toml");
    fs::write(
        &path,
        "[[query]]\nconnection = \"prod\"\nname = \"no sql\"\n\
         [[query]]\nconnection = \"prod\"\nname = \"ok\"\nsql = \"SELECT 1\"\n",
    )
    .unwrap();
    let store = QueryStore::at(path.clone());
    assert_eq!(names(&store, "prod"), ["ok"]);

    fs::write(&path, "query = 1\n").unwrap();
    let err = store.load("prod").unwrap_err();
    assert!(err.to_string().contains("not an array of tables"), "{err}");
    let err = store.save("prod", &query("x", "SELECT 1")).unwrap_err();
    assert!(err.to_string().contains("not an array of tables"), "{err}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "query = 1\n");
}
