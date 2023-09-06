use ethers::{
    providers::{Provider, Ws},
    types::{Address, H160, U256},
};
use log::info;
use std::{collections::HashMap, str::FromStr, sync::Arc};
use tokio::sync::broadcast::Sender;

use crate::streams::Event;

pub async fn arbitrage_event_handler(provider: Arc<Provider<Ws>>, event_sender: Sender<Event>) {}
