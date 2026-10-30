use serde::{Deserialize, Serialize};

use crate::StoredPath;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSidebarFavorite {
    pub label: String,
    pub path: StoredPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredNetworkConnection {
    pub id: String,
    pub label: String,
    pub protocol: String,
    pub uri: String,
    pub auto_connect: bool,
}
