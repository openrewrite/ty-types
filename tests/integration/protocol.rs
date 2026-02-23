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
    pub call_signature: Option<CallSignatureInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSignatureInfo {
    pub parameters: Vec<ParameterInfo>,
    pub return_type_id: Option<u32>,
    #[serde(default)]
    pub type_arguments: Vec<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterInfo {
    pub name: String,
    pub type_id: Option<u32>,
    pub kind: String,
    pub has_default: bool,
}
