#![warn(dead_code)]
#![warn(unused_imports)]


pub mod pb {
    tonic::include_proto!("fulcrum");
}

use tracing::{debug, error, Level};
// use tracing_subscriber::FmtSubscriber;
use tracing_attributes::instrument;
use tracing_futures;

// use std::hash::{Hash, Hasher};
use std::collections::HashSet;
use std::collections::VecDeque;

use prost::Message;
use sled::{Config as SledConfig};
use bytes::{Buf, IntoBuf};

// use futures::Stream;
use std::fmt;
use std::net::SocketAddr;
// use std::pin::Pin;
use tokio::sync::mpsc;
use tonic::{transport::Server, Request, Response, Status /*, Streaming*/};

use pb::*;
use pb::cdn_control_server::*;
use pb::cdn_query_server::*;

use sled::Db;

type GrpcResult<T> = Result<Response<T>, Status>;
// type ResponseStream = Pin<Box<dyn Stream<Item = Result<EchoResponse, Status>> + Send + Sync>>;

#[derive(Debug)]
pub struct CdnServer {
    addr: SocketAddr,
    db: Db
}


use internal_error::{*, Cause::*};


// impl Hash for CdnValue {
//     fn hash_slice<H: Hasher>(data: &[Self], state: &mut H)
//         where Self: Sized
//     {
//         for piece in data {
//             piece.hash(state);
//         }
//     }
//     fn hash<H: Hasher>(&self, state: &mut H) { 
//         self.message.hash(state);
//     }
// }

pub mod data_access {
    use std::fmt;

    use prost::Message;
    use bytes::{Buf, IntoBuf};
    
    use crate::pb::*;
    use crate::pb::cdn_control_server::*;
    use crate::pb::cdn_query_server::*;
    use internal_error::{*, Cause::*};

    use tracing::{debug, error, Level};

    use sled::Db;

    impl fmt::Display for CdnUid {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "CdnUid: {}", self.message)
        }
    }
    
    impl Eq for CdnUid {}    

    pub trait ProstMessageExt<T: ::prost::Message + Default> {
        fn to_bytes(self: &Self) -> Result<Vec<u8>, InternalError>;
        fn from_bytes<B: Buf>(msg_bytes: B) -> Result<T, InternalError>;
    }
    
    impl<T: ::prost::Message + Default> ProstMessageExt<T> for T {
        fn to_bytes(self: &Self) -> Result<Vec<u8>, InternalError> { 
            let mut msg_bytes = Vec::new();
            self.encode(&mut msg_bytes)
                .map_err(|e|
                    InternalError { cause: Some(StorageValueEncodingError(
                        EncodeError { required: e.required_capacity() as u64, remaining: e.remaining() as u64 } )) })?;
            Ok(msg_bytes)
        }
    
        fn from_bytes<B: Buf>(msg_bytes: B) -> Result<Self, InternalError> {
            let v = Self::decode(msg_bytes)
                .map_err(|e| {
                    let ee = Box::new(e) as Box<dyn std::error::Error>;
                    InternalError { cause: Some(StorageValueDecodingError(
                        DecodeError { description: ee.to_string(), stack: Vec::new()} )) } // TODO: Populate Stack
                })?;
            Ok(v)
        }
    }
    
    pub fn unwrap_field<T: ::prost::Message + Default>(msg: Option<T>, field_name: &str) -> Result<T, InternalError> { 
        msg.ok_or(InternalError { cause: Some(MissingRequiredArgument(field_name.to_string())) })
    }
    
    pub fn process_uid<T> (r_uid: Option<CdnUid>, f: impl FnOnce(&CdnUid, &Vec<u8>) -> Result<T, ::sled::Error>) -> Result<(CdnUid, T), InternalError> {
        let uid = unwrap_field(r_uid, "uid")?;
        let uid_bytes = uid.to_bytes()?;
    
        let old_value = f(&uid, &uid_bytes)
            .map_err(|e| InternalError { cause: Some(StorageError(e.to_string())) })?;
        Ok((uid, old_value))
    }

    pub enum GetResult {
        Success (CdnUid, CdnValue),
        NotFound (CdnUid),
        Error (InternalError)
    }

    pub fn get (db: &Db, key: Option<CdnUid>) -> GetResult {
        match process_uid(key, |_, uid_bytes| db.get(uid_bytes)) {
            Ok((uid, Some(v_bytes))) => {
                match CdnValue::from_bytes(v_bytes.into_buf()) {
                    Ok(v) => GetResult::Success(uid, v),
                    Err(e) => GetResult::Error(e),
                }
            },
            Ok((uid, None)) => GetResult::NotFound(uid),
            Err(e) => GetResult::Error(e)
        }
    }

    pub fn contains_key (db: &Db, key: Option<CdnUid>) -> Result<bool, InternalError> {
        process_uid(key, |_, uid_bytes| db.contains_key(uid_bytes)).map(|(_, v)| v)
    }

    pub enum DeleteResult {
        Success (CdnUid),
        NotFound (CdnUid),
        Error (InternalError)
    }
    
    pub fn delete (db: &Db, key: Option<CdnUid>) -> DeleteResult {
        match process_uid(key, |_, uid_bytes| db.remove(uid_bytes)) {
            Ok((uid, Some(_))) => DeleteResult::Success(uid),
            Ok((uid, None)) => DeleteResult::NotFound(uid),
            Err(e) => DeleteResult::Error(e)
        }
    }

    pub enum AddResult {
        Success (CdnUid),
        Exists (CdnUid),
        Error (InternalError)
    }
    
    pub fn add (db: &Db, key: Option<CdnUid>, value: Option<CdnValue>) -> AddResult {
        let res = || -> Result<AddResult, InternalError> {
            let val = unwrap_field(value, "value")?;
            let value_bytes = val.to_bytes()?;

            let check_and_insert = |uid: &CdnUid, uid_bytes: &Vec<u8>| -> Result<_, ::sled::Error> {
                let contains = db.contains_key(uid_bytes)?;
                if contains { 
                    Ok(AddResult::Exists(uid.clone()))
                }
                else {
                    let existing = db.insert(uid_bytes, value_bytes)?; 
                    if existing.is_some() {
                        error!("Unexpected override of the value in store: '{}'", uid); 
                    }
                    Ok(AddResult::Success(uid.clone()))
                } 
            };    
            
            let (_, ret) = process_uid(key, check_and_insert)?;
            Ok(ret)
        };
        
        match res() {
            Ok(resp) => resp,
            Err(e) => AddResult::Error(e)
        }
    }
}

use data_access::*;

#[tonic::async_trait]
impl CdnControl for CdnServer {

    #[instrument]
    async fn add(&self, request: Request<CdnAddRequest>) -> GrpcResult<CdnAddResponse> {
        use AddResult::*;
        type Resp = cdn_add_response::Resp;

        let r = request.into_inner();
        debug!("Add Received: '{:?}':'{:?}' (from {})", r.uid, r.value, self.addr);        
        
        let res = match add(&self.db, r.uid, r.value) {
            Success(uid) => Resp::Success(uid),
            Exists(_) => Resp::Exists(()), 
            Error(e) => Resp::Error(e)
        };

        Ok(Response::new(CdnAddResponse { resp: Some(res) }))
    }

    async fn delete(&self, request: Request<CdnDeleteRequest>) -> GrpcResult<CdnDeleteResponse> {
        use DeleteResult::*;
        type Resp = cdn_delete_response::Resp;

        let r = request.into_inner();
        debug!("'{:?}' (from {})", r.uid, self.addr);
        
        let res = match delete(&self.db, r.uid) {
            Success(uid) => Resp::Success(uid),
            NotFound(_) => Resp::NotFound(()), 
            Error(e) => Resp::Error(e)
        };

        Ok(Response::new(CdnDeleteResponse { resp: Some(res) }))
    }
}


type StreamValueStreamSender = mpsc::Sender<Result<CdnStreamValueResponse, Status>>;

//#[instrument]
async fn send_response_msg (tx: &mut StreamValueStreamSender, resp: cdn_stream_value_response::Resp) {
    let msg = Ok(CdnStreamValueResponse { resp: Some(resp) });
    debug!("StreamValueStream sending: {:?}", &msg);
    match tx.send(msg).await {
        Ok(()) => (),
        Err(e) => error!("Value message transfer failed with: {}", e)
    }
}

#[tonic::async_trait]
impl CdnQuery for CdnServer {

    // #[instrument(level = "debug")]
    #[instrument]
    async fn get(&self, request: Request<CdnGetRequest>) -> GrpcResult<CdnGetResponse> {
        use GetResult::*;
        type Resp = cdn_get_response::Resp;

        let r = request.into_inner();
        debug!("Get Received: '{:?}' (from {})", r.uid, self.addr); // TODO: Fix tracing and remove
        
        let res = match get(&self.db, r.uid) {
            Success(uid, v) => Resp::Success(v),
            NotFound(_) => Resp::NotFound(()), 
            Error(e) => Resp::Error(e)
        };

        Ok(Response::new(CdnGetResponse { resp: Some(res) }))
    }

    async fn contains(&self, request: Request<CdnContainsRequest>) -> GrpcResult<CdnContainsResponse> {
        let r = request.into_inner();
        debug!("Contains Received: '{:?}' (from {})", r.uid, self.addr);

        let res = match contains_key(&self.db, r.uid) {
            Ok(v) => cdn_contains_response::Resp::Success(v),
            Err(e) => cdn_contains_response::Resp::Error(e)
        };

        Ok(Response::new(CdnContainsResponse { resp: Some(res) }))
    }

    type StreamValueStream = mpsc::Receiver<Result<CdnStreamValueResponse, Status>>;
    async fn stream_value(&self, request: Request<CdnGetRequest>) -> GrpcResult<Self::StreamValueStream> {
        let r = request.into_inner();
        let message = format!("'{:?}' (from {})", r.uid, self.addr);
        println!("StreamValueStream Received: {}", message);

        let key = r.uid.clone();

        let (mut tx, rx) = mpsc::channel(4);
        let db = self.db.clone();

        async fn get_kv(db: &Db, tx: &mut StreamValueStreamSender, key: Option<CdnUid>) -> Vec<CdnUid> {
            match get(db, key) {
                GetResult::Success(uid, v) => {
                    let msg = &v.message;
                    match msg {
                        Some(cdn_value::Message::Batch(cdn_value::Batch { uids })) => {
                            send_response_msg(tx, cdn_stream_value_response::Resp::Success(CdnKeyValue { key: Some(uid), value: Some(v.clone()) })).await;
                            uids.clone()
                        },
                        Some(cdn_value::Message::Bytes(_)) => {
                            send_response_msg(tx, cdn_stream_value_response::Resp::Success(CdnKeyValue { key: Some(uid), value: Some(v.clone()) })).await;
                            Vec::new()
                        },
                        None => {
                            let e = InternalError { cause: Some(StorageValueDecodingError( DecodeError { description: "'message' field is required".to_string(), stack: vec![ internal_error::decode_error::StackLine { message: "field is required".to_string(), field: "message".to_string() } ] })) };
                            error! ("uid: {}, '{:?}'", uid, e);
                            send_response_msg(tx, cdn_stream_value_response::Resp::Error(e)).await;
                            Vec::new()
                        }
                    }
                },
                GetResult::NotFound(_) => { 
                    send_response_msg(tx, cdn_stream_value_response::Resp::NotFound(())).await;
                    Vec::new()
                }, 
                GetResult::Error(e) => {
                    send_response_msg(tx, cdn_stream_value_response::Resp::Error(e)).await;
                    Vec::new()
                }
            }
        }

        tokio::spawn(async move {
            let mut seen = HashSet::<String>::new(); // TODO: implement Hash for CdnUid
            let mut remaining_keys = VecDeque::new();
            remaining_keys.push_back(key);
            
            while 
                match remaining_keys.pop_front() { 
                    Some (next_key) => {
                        let keys = get_kv(&db, &mut tx, next_key).await;
                        for k in keys {
                            if !seen.contains(&k.message) {
                                seen.insert(k.message.clone());
                                remaining_keys.push_back(Some(k));
                            }
                        }
                        true
                    },
                    None => false
                }
            {
            }
        });

        Ok(Response::new(rx))
    }
}

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

    for addr in &addrs {
        let addr = addr.parse()?;
        let tx = tx.clone();

        let control_server = CdnServer { addr, db: db.clone() };
        let query_server = CdnServer { addr, db: db.clone() };
        let serve = Server::builder()
            .add_service(pb::cdn_control_server::CdnControlServer::new(control_server))
            .add_service(pb::cdn_query_server::CdnQueryServer::new(query_server))
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