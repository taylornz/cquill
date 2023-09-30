use std::{path::PathBuf, str};

use anyhow::{anyhow, Result};
use scylla::Session;

pub use crate::cql::CqlFile;
use crate::keyspace::*;
pub use crate::migrate::{MigrateError, MigrateErrorState};
use crate::queries::*;

mod cql;
pub mod keyspace;
mod migrate;
mod queries;
#[cfg(test)]
pub(crate) mod test_utils;

const NODE_ADDRESS: &str = "127.0.0.1:9042";

pub const KEYSPACE: &str = "cquill";

pub const TABLE: &str = "migrated_cql";

pub struct MigrateOpts {
    pub cassandra_opts: Option<CassandraOpts>,
    pub cql_dir: PathBuf,
    pub apply_keyspace: String,
    pub history_keyspace: Option<KeyspaceOpts>,
    pub history_table: Option<String>,
}

#[derive(Default)]
pub struct CassandraOpts {
    pub cassandra_host: Option<String>,
}

impl CassandraOpts {
    pub fn node_address(&self) -> String {
        let node_address = match &self.cassandra_host {
            None => std::env::var("CASSANDRA_NODE").unwrap_or(NODE_ADDRESS.to_string()),
            Some(cassandra_host) => cassandra_host.clone(),
        };
        if node_address.contains(':') {
            node_address
        } else {
            format!("{node_address}:9042")
        }
    }
}

/// `migrate_cql` performs a migration of all newly added cql scripts in [MigrateOpts::cql_dir]
/// since its last invocation. Migrated scripts are tracked in a cquill keyspace and history table
/// specified with [MigrateOpts::history_keyspace] and [MigrateOpts::history_table]. A successful
/// method result contains a vec of the cql script paths executed during this invocation.
pub async fn migrate_cql(opts: MigrateOpts) -> Result<Vec<CqlFile>, MigrateError> {
    let cql_files = cql::files_from_dir(&opts.cql_dir)?;
    let node_address = opts.cassandra_opts.unwrap_or_default().node_address();
    let session = cql_session(node_address).await?;

    let cquill_keyspace = opts
        .history_keyspace
        .unwrap_or_else(|| KeyspaceOpts::simple(String::from(KEYSPACE), 1));
    let history_table = opts.history_table.unwrap_or_else(|| String::from(TABLE));
    prepare_cquill_keyspace(&session, &cquill_keyspace, &history_table).await?;

    migrate::perform(
        &session,
        &cql_files,
        migrate::MigrateArgs {
            cql_dir: opts.cql_dir,
            apply_keyspace: opts.apply_keyspace,
            history_keyspace: cquill_keyspace.name,
            history_table,
        },
    )
    .await
}

// todo drop and recreate dev mode
async fn prepare_cquill_keyspace(
    session: &Session,
    keyspace: &KeyspaceOpts,
    table_name: &String,
) -> Result<()> {
    let create_table: bool = match table_names_from_session_metadata(session, &keyspace.name) {
        Ok(table_names) => !table_names.contains(table_name),
        Err(_) => {
            queries::keyspace::create(session, keyspace).await?;
            true
        }
    };
    if create_table {
        migrated::table::create(session, &keyspace.name, table_name).await?;
    }
    Ok(())
}

async fn cql_session(node_address: String) -> Result<Session> {
    let connecting = scylla::SessionBuilder::new()
        .known_node(&node_address)
        .build()
        .await;
    match connecting {
        Ok(session) => Ok(session),
        Err(_) => Err(anyhow!("could not connect to {}", &node_address)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cassandra_opts_provides_node_address() {
        let without_host = CassandraOpts {
            cassandra_host: None,
        };
        let with_host = CassandraOpts {
            cassandra_host: Some("localhost".to_string()),
        };
        let with_port = CassandraOpts {
            cassandra_host: Some("localhost:9043".to_string()),
        };
        assert_eq!(
            without_host.node_address(),
            std::env::var("CASSANDRA_NODE").unwrap_or(NODE_ADDRESS.to_string())
        );
        assert_eq!(with_host.node_address(), "localhost:9042");
        assert_eq!(with_port.node_address(), "localhost:9043");
    }

    #[tokio::test]
    async fn test_prepare_cquill_keyspace_when_keyspace_does_not_exist() {
        let session = test_utils::cql_session().await;
        let keyspace_opts = KeyspaceOpts::simple(test_utils::keyspace_name(), 1);
        let table_name = String::from("table_name");

        if let Err(err) = prepare_cquill_keyspace(&session, &keyspace_opts, &table_name).await {
            println!("{err}");
            panic!();
        }
        match table_names_from_session_metadata(&session, &keyspace_opts.name) {
            Ok(table_names) => assert!(table_names.contains(&table_name)),
            Err(_) => panic!(),
        }

        queries::keyspace::drop(&session, &keyspace_opts.name)
            .await
            .expect("drop keyspace");
    }

    #[tokio::test]
    async fn test_prepare_cquill_keyspace_when_table_does_not_exist() {
        let session = test_utils::cql_session().await;
        let keyspace_opts = test_utils::create_keyspace(&session).await;
        let table_name = String::from("table_name");

        prepare_cquill_keyspace(&session, &keyspace_opts, &table_name)
            .await
            .expect("prepare keyspace");
        match table_names_from_session_metadata(&session, &keyspace_opts.name) {
            Ok(table_names) => assert!(table_names.contains(&table_name)),
            Err(_) => panic!(),
        }

        queries::keyspace::drop(&session, &keyspace_opts.name)
            .await
            .expect("drop keyspace");
    }

    #[tokio::test]
    async fn test_prepare_cquill_keyspace_when_keyspace_and_table_exist() {
        let harness = test_utils::TestHarness::builder().initialize().await;

        prepare_cquill_keyspace(
            &harness.session,
            &KeyspaceOpts::simple(harness.cquill_keyspace.clone(), 1),
            &harness.cquill_table,
        )
        .await
        .expect("prepare keyspace");
        match table_names_from_session_metadata(&harness.session, &harness.cquill_keyspace) {
            Ok(table_names) => assert!(table_names.contains(&harness.cquill_table)),
            Err(_) => panic!(),
        }

        harness.drop_keyspace().await;
    }
}
