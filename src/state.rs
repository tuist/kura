use std::{collections::BTreeSet, sync::Arc};

use reqwest::Client;
use tokio::sync::{Notify, RwLock};

use crate::{
    config::Config, io::IoController, memory::MemoryController, metrics::Metrics, store::Store,
};

pub struct AppState {
    pub config: Config,
    pub store: Store,
    pub io: IoController,
    pub memory: MemoryController,
    pub metrics: Metrics,
    pub client: Client,
    pub notify: Notify,
    pub members: RwLock<BTreeSet<String>>,
}

pub type SharedState = Arc<AppState>;
