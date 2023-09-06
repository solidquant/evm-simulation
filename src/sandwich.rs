use ethers::{
    providers::{Provider, Ws},
    types::{Address, H160, U256},
};
use log::info;
use std::{collections::HashMap, str::FromStr, sync::Arc};
use tokio::sync::broadcast::Sender;

use crate::streams::Event;

pub async fn sandwich_event_handler(provider: Arc<Provider<Ws>>, event_sender: Sender<Event>) {
    let mut event_receiver = event_sender.subscribe();

    loop {
        match event_receiver.recv().await {
            Ok(event) => match event {
                Event::Block(block) => {
                    info!("{:?}", block);
                }
                Event::PendingTx(tx) => {
                    info!("{:?}", tx.hash);
                }
                Event::Log(_) => {}
            },
            Err(_) => {}
        }
    }
}
