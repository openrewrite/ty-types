use serde::Deserialize;
use std::collections::HashMap;

pub type TypeMap = HashMap<String, serde_json::Value>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub start: u32,
    pub end: u32,
    pub node_kind: String,
    pub type_id: Option<u32>,
}
