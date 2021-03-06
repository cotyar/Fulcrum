#![warn(dead_code)]
#![warn(unused_imports)]


pub mod pb {
    tonic::include_proto!("fulcrum");
}

use tracing::{Level};
use sled::Config as SledConfig;

// use futures::Stream;
use tokio::sync::mpsc;
use tonic::{transport::Server, /*, Streaming*/};

mod error_handling;

mod data_access;

mod cdn;
use cdn::*;

mod data_tree;
use data_tree::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::Subscriber::builder()
        // all spans/events with a level higher than DEBUG (e.g, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(Level::DEBUG)
        // .with_env_filter("attrs_basic=trace")
        // sets this to be the default, global subscriber for this application.
        .init();

    let addrs = ["[::1]:50151", "[::1]:50152"];

    let (tx, mut rx) = mpsc::unbounded_channel();

    let config = SledConfig::new().temporary(true);
    let db = config.open()?;
    let cdn_tree = db.open_tree("cdn")?;
    let data_tree = db.open_tree("data")?;

    for addr in &addrs {
        let addr = addr.parse()?;
        let tx = tx.clone();

        let cdn_control_server = CdnServer { addr, tree: cdn_tree.clone() };
        let cdn_query_server = CdnServer { addr, tree: cdn_tree.clone() };
        let data_tree_server = DataTreeServer { addr, tree: KeyColumn::SimpleKeyColumn(data_tree.clone()) };
        let serve = Server::builder()
            .add_service(pb::cdn_control_server::CdnControlServer::new(cdn_control_server))
            .add_service(pb::cdn_query_server::CdnQueryServer::new(cdn_query_server))
            .add_service(pb::data_tree_server::DataTreeServer::new(data_tree_server))
            .serve(addr);

        tokio::spawn(async move {
            if let Err(e) = serve.await {
                eprintln!("Error = {:?}", e);
            }

            tx.send(()).unwrap();
        });
    }

    rx.recv().await;

    Ok(())
}