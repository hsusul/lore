//! Every foreign key's child columns are indexed. SQLite looks up child rows
//! whenever a parent row is deleted, and on every parent insert while a
//! deferred constraint is pending. Without an index each lookup scans the
//! whole child table, so ingest time grew with the square of archive size.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use rusqlite::Connection;

fn strings(conn: &Connection, sql: &str) -> Vec<String> {
    conn.prepare(sql)
        .unwrap()
        .query_map([], |r| r.get::<_, Option<String>>(0))
        .unwrap()
        .map(|name| name.unwrap().unwrap_or_default())
        .collect()
}

#[test]
fn every_foreign_key_has_an_index_on_its_child_columns() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let tables = strings(
        &conn,
        "SELECT name FROM sqlite_master WHERE type = 'table'
         AND name NOT LIKE 'sqlite_%' AND sql NOT LIKE 'CREATE VIRTUAL%' ORDER BY name",
    );
    let mut unindexed = Vec::new();
    for table in &tables {
        let mut leading: Vec<Vec<String>> = strings(
            &conn,
            &format!("SELECT name FROM pragma_index_list('{table}')"),
        )
        .iter()
        .map(|index| {
            strings(
                &conn,
                &format!("SELECT name FROM pragma_index_info('{index}') ORDER BY seqno"),
            )
        })
        .collect();
        leading.push(strings(
            &conn,
            &format!("SELECT name FROM pragma_table_info('{table}') WHERE pk > 0 ORDER BY pk"),
        ));
        let keys: Vec<(i64, String, String)> = conn
            .prepare(&format!(
                "SELECT id, \"from\", \"table\" FROM pragma_foreign_key_list('{table}') ORDER BY id, seq"
            ))
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let mut ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        ids.dedup();
        for id in ids {
            let columns: Vec<String> = keys
                .iter()
                .filter(|k| k.0 == id)
                .map(|k| k.1.clone())
                .collect();
            let parent = &keys.iter().find(|k| k.0 == id).unwrap().2;
            if !leading.iter().any(|index| index.starts_with(&columns)) {
                unindexed.push(format!("{table}({}) -> {parent}", columns.join(", ")));
            }
        }
    }
    assert_eq!(unindexed, Vec::<String>::new());
}
