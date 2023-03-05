use crate::keyspace::ReplicationFactor::*;
use anyhow::anyhow;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;
use std::str::{FromStr, Split};

pub const REPLICATION: &str = "{ 'class': 'SimpleStrategy', 'replication_factor': 1 }";

/// KeyspaceOpts describes a keyspace managed by cquill with a keyspace name and
/// [ReplicationFactor].
pub struct KeyspaceOpts {
    pub name: String,
    /// The keyspace [ReplicationFactor] will default to a development environment setting using
    /// SimpleStrategy with a replication factor of 1.
    pub replication: Option<ReplicationFactor>,
}

impl KeyspaceOpts {
    pub fn simple(name: String, factor: u8) -> Self {
        KeyspaceOpts {
            name,
            replication: Some(SimpleStrategy { factor }),
        }
    }
}

/// ReplicationFactor represents the strategy and data replication factor for a keyspace.
pub enum ReplicationFactor {
    /// NetworkTopologyStrategy specifies how many replications will be placed in specific
    /// datacenters within the cluster.
    NetworkTopologyStrategy {
        datacenter_factors: HashMap<String, u8>,
    },
    /// SimpleStrategy specifies a single number of replications distributed throughout any nodes
    /// within the cluster. This strategy does not provide sufficient resiliency and fault tolerance
    /// and should not be used with production systems.
    SimpleStrategy { factor: u8 },
}

impl FromStr for ReplicationFactor {
    type Err = anyhow::Error;

    /// from_str performs a manual deserialization of a `CREATE KEYSPACE` statement's replication
    /// settings from the CQL key-value hash object. Valid input from the CLI default can be seen
    /// in [REPLICATION].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == REPLICATION {
            return Ok(SimpleStrategy { factor: 1 });
        }
        let trimmed = s.trim();
        if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
            return Err(anyhow!("not a valid keyspace replication object"));
        }
        // collect all key value pairs from {} object into a HashMap
        let mut fields: HashMap<String, String> = HashMap::new();
        let fields_split = trimmed[1..trimmed.len() - 1].split(',');
        for value_pair in fields_split {
            let mut key_value_split = value_pair.split(':');
            let next_from_key_value_split = |key_value_split: &mut Split<char>| -> Option<String> {
                key_value_split
                    .next()
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
            };
            let maybe_key = next_from_key_value_split(&mut key_value_split);
            let maybe_value = next_from_key_value_split(&mut key_value_split);
            match (maybe_key, maybe_value) {
                (Some(key), Some(value)) => {
                    if fields.insert(key.clone(), value).is_some() {
                        return Err(anyhow!(
                            "replication object duplicates key-value pair {key}"
                        ));
                    }
                }
                (_, _) => {
                    return Err(anyhow!(
                        "not a valid key-value pair in keyspace replication object"
                    ))
                }
            }
        }
        match fields.remove("class") {
            None => Err(anyhow!("replication object missing class field")),
            Some(replication_class) => match replication_class.as_str() {
                "NetworkTopologyStrategy" => {
                    if fields.is_empty() {
                        return Err(anyhow!("network replication must specify at least one datacenter's replication factor"));
                    }
                    let mut datacenter_factors: HashMap<String, u8> = HashMap::new();
                    lazy_static! {
                        static ref DATACENTER_REGEX: Regex =
                            regex::Regex::new(r"^[a-z\d_]{2,}$").unwrap();
                    }
                    for (datacenter, factor_string) in fields.iter() {
                        if !DATACENTER_REGEX.is_match(datacenter) {
                            return Err(anyhow!("datacenter {datacenter} is not a valid name"));
                        }
                        match factor_string.parse::<u8>() {
                            Ok(factor) => {
                                datacenter_factors.insert(datacenter.clone(), factor);
                            }
                            Err(_) => return Err(anyhow!("replication factor {datacenter} for datacenter {factor_string} must be a number"))
                        }
                    }
                    Ok(NetworkTopologyStrategy { datacenter_factors })
                }
                "SimpleStrategy" => match fields.get("replication_factor") {
                    Some(factor_string) => match factor_string.parse::<u8>() {
                        Ok(factor) => Ok(SimpleStrategy { factor }),
                        Err(_) => Err(anyhow!(
                            "replication factor {factor_string} must be a number"
                        )),
                    },
                    None => Err(anyhow!(
                        "replication object missing replication_factor field"
                    )),
                },
                _ => Err(anyhow!(
                    "replication class {replication_class} field is an unsupported type"
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replication_factory_from_str_simple_default() {
        let result = REPLICATION.parse::<ReplicationFactor>();
        assert!(result.is_ok());
        let rep_factor = result.unwrap();
        match rep_factor {
            NetworkTopologyStrategy { .. } => panic!(),
            SimpleStrategy { factor } => assert_eq!(factor, 1),
        }
    }

    #[test]
    fn test_replication_factory_from_str_simple_custom() {
        let replication_factor = "{ 'class': 'SimpleStrategy', 'replication_factor': 3 }";
        let result = replication_factor.parse::<ReplicationFactor>();
        assert!(result.is_ok());
        let rep_factor = result.unwrap();
        match rep_factor {
            NetworkTopologyStrategy { .. } => panic!(),
            SimpleStrategy { factor } => assert_eq!(factor, 3),
        }
    }

    #[test]
    fn test_replication_factory_from_str_network() {
        let replication_factor = "{ 'class': 'NetworkTopologyStrategy', 'dc1': 3, 'dc2': 5 }";
        let result = replication_factor.parse::<ReplicationFactor>();
        match result {
            Ok(_) => {
                let rep_factor = result.unwrap();
                match rep_factor {
                    NetworkTopologyStrategy { datacenter_factors } => {
                        assert_eq!(datacenter_factors.get("dc1").unwrap().clone(), 3);
                        assert_eq!(datacenter_factors.get("dc2").unwrap().clone(), 5);
                    }
                    SimpleStrategy { .. } => panic!(),
                }
            }
            Err(_) => panic!(),
        }
    }

    fn test_replication_factory_from_str_error(input: &str, err_msg: &str) {
        let result = input.parse::<ReplicationFactor>();
        match result {
            Ok(_) => panic!(),
            Err(err) => assert_eq!(err.to_string(), err_msg),
        }
    }

    #[test]
    fn test_replication_factory_from_str_error_not_key_value_object() {
        test_replication_factory_from_str_error(
            "you're killing me, smalls",
            "not a valid keyspace replication object",
        );
    }

    #[test]
    fn test_replication_factory_from_str_error_no_key_value_pairs_in_object() {
        test_replication_factory_from_str_error(
            "{not, valid}",
            "not a valid key-value pair in keyspace replication object",
        );
    }

    #[test]
    fn test_replication_factory_from_str_error_no_replication_class() {
        test_replication_factory_from_str_error(
            "{something: else}",
            "replication object missing class field",
        );
    }

    #[test]
    fn test_replication_factory_from_str_error_unsupported_replication_class() {
        test_replication_factory_from_str_error(
            "{'class': 'FooStrategy'}",
            "replication class FooStrategy field is an unsupported type",
        );
    }

    #[test]
    fn test_replication_factory_from_str_error_simple_without_factor() {
        test_replication_factory_from_str_error(
            "{'class': 'SimpleStrategy'}",
            "replication object missing replication_factor field",
        );
    }

    #[test]
    fn test_replication_factory_from_str_error_simple_factor_not_a_number() {
        test_replication_factory_from_str_error(
            "{'class': 'SimpleStrategy', 'replication_factor': 'abc'}",
            "replication factor abc must be a number",
        );
    }

    #[test]
    fn test_replication_factory_from_str_error_network_without_factor() {
        test_replication_factory_from_str_error(
            "{'class': 'NetworkTopologyStrategy'}",
            "network replication must specify at least one datacenter's replication factor",
        );
    }

    #[test]
    fn test_replication_factory_from_str_error_duplicates_key_value_pair() {
        test_replication_factory_from_str_error(
            "{'class': 'NetworkTopologyStrategy', 'dc1': 1, 'dc1': 1}",
            "replication object duplicates key-value pair dc1",
        );
    }

    #[test]
    fn test_replication_factory_from_str_error_network_factor_bad_dc_name() {
        test_replication_factory_from_str_error(
            "{'class': 'NetworkTopologyStrategy', 'my datacenter': 3}",
            "datacenter my datacenter is not a valid name",
        );
    }
}
