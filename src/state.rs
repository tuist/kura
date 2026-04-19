use std::{collections::BTreeSet, sync::Arc};

use reqwest::Client;
use tokio::sync::{Notify, RwLock};

use crate::{config::Config, metrics::Metrics, store::Store};

pub struct AppState {
    pub config: Config,
    pub store: Store,
    pub metrics: Metrics,
    pub client: Client,
    pub notify: Notify,
    pub members: RwLock<BTreeSet<String>>,
}

pub type SharedState = Arc<AppState>;
