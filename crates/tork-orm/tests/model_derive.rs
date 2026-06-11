//! Tests for `#[derive(Model)]`: the generated metadata, row mapping, and the
//! insert/primary-key accessors.

use tork_orm::prelude::*;

#[derive(Debug, Clone, Model)]
#[table(name = "users")]
struct User {
    #[field(primary_key, auto)]
    id: i64,
    #[field(varchar(length = 50))]
    username: String,
    #[field(varchar(length = 255))]
    email: String,
    is_active: bool,
    nickname: Option<String>,
}

#[derive(Debug, Clone, Model)]
#[table(name = "posts")]
struct Post {
    #[field(primary_key, auto)]
    id: i64,
    #[field(foreign_key = User::id)]
    user_id: i64,
    title: String,
    view_count: i64,
}

#[test]
fn table_and_primary_key_metadata() {
    assert_eq!(<User as Model>::TABLE, "users");
    assert_eq!(<User as Model>::PRIMARY_KEY, "id");
    assert_eq!(<Post as Model>::TABLE, "posts");
}

#[test]
fn column_metadata_is_complete() {
    let columns = <User as Model>::COLUMNS;
    assert_eq!(columns.len(), 5);

    assert_eq!(columns[0].name, "id");
    assert!(columns[0].primary_key);
    assert!(columns[0].auto);
    assert_eq!(columns[0].sql_type, SqlType::BigInt);
    assert!(!columns[0].nullable);

    assert_eq!(columns[1].name, "username");
    assert_eq!(columns[1].sql_type, SqlType::Varchar(50));

    let nickname = columns.iter().find(|c| c.name == "nickname").unwrap();
    assert!(nickname.nullable);
    assert_eq!(nickname.sql_type, SqlType::Text);
}

#[test]
fn foreign_key_resolves_to_referenced_table() {
    let columns = <Post as Model>::COLUMNS;
    let user_id = columns.iter().find(|c| c.name == "user_id").unwrap();
    let fk = user_id.foreign_key.expect("user_id has a foreign key");
    assert_eq!(fk.table, "users");
    assert_eq!(fk.column, "id");
}

#[test]
fn from_row_builds_an_instance_by_column_name() {
    // Column order differs from field order to prove name-based mapping.
    let row = Row::new(
        vec![
            "is_active".into(),
            "email".into(),
            "id".into(),
            "username".into(),
            "nickname".into(),
        ],
        vec![
            Value::Bool(true),
            Value::Text("alice@example.com".into()),
            Value::Int(7),
            Value::Text("alice".into()),
            Value::Null,
        ],
    );

    let user = User::from_row(&row).unwrap();
    assert_eq!(user.id, 7);
    assert_eq!(user.username, "alice");
    assert_eq!(user.email, "alice@example.com");
    assert!(user.is_active);
    assert_eq!(user.nickname, None);
}

#[test]
fn insert_values_skip_auto_primary_key() {
    let user = User {
        id: 0,
        username: "bob".into(),
        email: "bob@example.com".into(),
        is_active: false,
        nickname: Some("bobby".into()),
    };

    let values = user.insert_values();
    let names: Vec<&str> = values.iter().map(|(name, _)| *name).collect();
    // `id` is auto, so it is omitted; every other column is present.
    assert_eq!(names, ["username", "email", "is_active", "nickname"]);
    assert_eq!(values[0].1, Value::Text("bob".into()));
    assert_eq!(values[3].1, Value::Text("bobby".into()));
}

#[test]
fn primary_key_value_reads_the_pk_field() {
    let user = User {
        id: 42,
        username: "carol".into(),
        email: "carol@example.com".into(),
        is_active: true,
        nickname: None,
    };
    assert_eq!(user.primary_key_value(), Value::Int(42));
}
