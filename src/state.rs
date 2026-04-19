use std::{collections::BTreeSet, sync::Arc};

use reqwest::Client;
use tokio::sync::{Notify, RwLock};

use crate::{config::Config, metrics::Metrics, store::Store};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Arc<Store>,
    pub metrics: Arc<Metrics>,
    pub client: Client,
    pub notify: Arc<Notify>,
    pub members: Arc<RwLock<BTreeSet<String>>>,
}

pub type SharedState = Arc<AppState>;
