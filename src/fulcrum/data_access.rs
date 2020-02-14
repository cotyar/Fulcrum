#![warn(dead_code)]
#![warn(unused_imports)]

use core::hash::{Hash, Hasher};
use core::mem::size_of;
use core::future::Future;
use tokio::sync::{mpsc, mpsc::*};
use std::fmt;
use std::sync::Arc;
use std::thread;
use crossbeam_epoch::{self as epoch, Atomic, Owned};

use bytes::{Bytes, Buf};

extern crate async_trait;
use async_trait::async_trait;

use crate::pb::*;
use internal_error::{*, Cause::*};

use tracing::{debug, error, Level};

use sled::{IVec, Tree, TransactionalTree};
//use sled::{TransactionError, TransactionResult, abort, Transactional, ConflictableTransactionResult, ConflictableTransactionError};

pub trait ProstMessage : ::prost::Message + Default {}
pub trait Uid : fmt::Debug + Send + Sync + Clone + fmt::Display {
    fn to_key_bytes(self: &Self) -> Result<Vec<u8>, InternalError>;
    fn from_key_bytes<B: Buf>(mut msg_bytes: B) -> Result<Self, InternalError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyU64(pub u64);

impl Uid for KeyU64 {
    fn to_key_bytes(self: &Self) -> Result<Vec<u8>, InternalError> {
        Ok(self.0.to_be_bytes().to_vec())
    }
    fn from_key_bytes<B: Buf>(mut msg_bytes: B) -> Result<Self, InternalError> {
        Ok(KeyU64(msg_bytes.get_u64()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyString(pub String);

impl Hash for KeyString {
    fn hash_slice<H: Hasher>(data: &[Self], state: &mut H) where Self: Sized
    {
        for piece in data {
            state.write(piece.0.as_bytes());
        }
    }
    fn hash<H: Hasher>(&self, state: &mut H) { 
        state.write(self.0.as_bytes());
    }
}

impl Uid for KeyString {
    fn to_key_bytes(self: &Self) -> Result<Vec<u8>, InternalError> {
        Ok(self.0.bytes().collect())
    }
    fn from_key_bytes<B: Buf>(msg_bytes: B) -> Result<Self, InternalError> {
        String::from_utf8(msg_bytes.bytes().iter().cloned().collect()).
            map_err(|e| InternalError { cause: Some(StorageValueDecodingError(DecodeError{ description: e.to_string(), stack: Vec::new() }))}).
            map(|s| KeyString(s))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyVec(Vec<u8>);

impl Uid for KeyVec {
    fn to_key_bytes(self: &Self) -> Result<Vec<u8>, InternalError> {
        Ok(self.0.clone())
    }
    fn from_key_bytes<B: Buf>(mut msg_bytes: B) -> Result<KeyVec, InternalError> {
        Ok(KeyVec(msg_bytes.bytes().iter().cloned().collect()))
    }
}

impl Eq for CdnUid {} 
impl ProstMessage for CdnUid {} 
impl Uid for CdnUid {
    fn to_key_bytes(self: &Self) -> Result<Vec<u8>, InternalError> {
        self.to_bytes()
    }
    fn from_key_bytes<B: Buf>(msg_bytes: B) -> Result<CdnUid, InternalError> {
        CdnUid::from_bytes(msg_bytes)
    }
}

impl Eq for KeyUid {} 
impl ProstMessage for KeyUid {}
impl Uid for KeyUid {
    fn to_key_bytes(self: &Self) -> Result<Vec<u8>, InternalError> {
        self.to_bytes()
    }
    fn from_key_bytes<B: Buf>(msg_bytes: B) -> Result<KeyUid, InternalError> {
        KeyUid::from_bytes(msg_bytes)
    }
} // TODO: Revise Key structure

impl ProstMessage for CdnValue {}
impl ProstMessage for ValueEntry {}
impl ProstMessage for KvEntry {}

impl fmt::Display for CdnUid {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Uid: {}", self.message)
    }
}

impl fmt::Display for KeyUid {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "KeyUid: {:?}", self)
    }
}

impl fmt::Display for KeyU64 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "KeyU64: {:?}", self)
    }
}

impl fmt::Display for KeyString {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "KeyString: {:?}", self)
    }
}


impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Key: {}, KeyUid: {:?}, KeyFamily: {:?}", self.key, self.uid, self.key_family)
    }
}

impl fmt::Display for KeyVec {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}


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

pub trait ProstMessageExt where Self: Sized {
    fn to_bytes(self: &Self) -> Result<Vec<u8>, InternalError>;
    fn from_bytes<B: Buf>(msg_bytes: B) -> Result<Self, InternalError>;
}

impl<T: ProstMessage> ProstMessageExt for T {
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

// fn to_internal_error (e: ConflictableTransactionError<::sled::Error>) -> InternalError {
//     InternalError { cause: Some(StorageError(e.to_string())) }
// }

pub fn unwrap_field<T>(msg: Option<T>, field_name: &str) -> Result<T, InternalError> { 
    msg.ok_or(InternalError { cause: Some(MissingRequiredArgument(field_name.to_string())) })
}

pub fn process_uid<T: Uid, U> (r_uid: Option<T>, f: impl FnOnce(&T, &Vec<u8>) -> Result<U, InternalError>) -> Result<(T, U), InternalError> {
    let uid = unwrap_field(r_uid, "uid")?;
    let uid_bytes = uid.to_key_bytes()?;

    let old_value = f(&uid, &uid_bytes)?;
    Ok((uid, old_value))
}

// impl fmt::Debug for InternalError {
//     fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
//         let msg = format_internal_error(self);
//         write!(f, "{{ An Error Occurred: '{:?}', file: {}, line: {} }}", msg, file!(), line!(), ) // programmer-facing output
//     }
// }

#[derive(Debug)]
pub enum GetResultSuccess<T: Uid, U: ProstMessage> {
    Success (T, U),
    NotFound (T),
}

pub type GetResult<T: Uid, U: ProstMessage> =
    std::result::Result<GetResultSuccess<T, U>, InternalError>;

pub fn get<T: Uid, U: ProstMessage> (tree: &Tree, key: Option<T>) -> GetResult<T, U> {
    let (uid, v_bytes_opt) = process_uid(key, |_, uid_bytes| { 
            let v = tree.get(uid_bytes)?;
            Ok(v) 
        })?;
    match v_bytes_opt {
        Some (v_bytes) => {
            let bts = v_bytes.to_vec();
            let v = U::from_bytes(Bytes::from(bts))?;
            Ok(GetResultSuccess::Success(uid, v))
        },
        None => Ok(GetResultSuccess::NotFound(uid))
    }
}

pub fn contains_key<T: Uid> (tree: &Tree, key: Option<T>) -> Result<bool, InternalError> {
    process_uid(key, |_, uid_bytes| { 
        let v = tree.contains_key(uid_bytes)?;
        Ok(v)
     }).map(|(_, v)| v)
}

#[derive(Debug, Clone)]
pub enum PageResult<U: 'static + Send> {
    Success (U),
    KeyError (InternalError),
    ValueError (InternalError)
}

#[async_trait]
pub trait Pager where Self: Uid {
    async fn get_page_by_prefix<U: Clone + Send + fmt::Debug +'static> 
        (tree: &Tree, buffer_size: usize, key: Option<Self>, page: Option<u32>, page_size: Option<u32>, default_page_size: u32, 
            f: Box<dyn Fn((sled::IVec, sled::IVec)) -> PageResult<U> + Send>)
            -> Result<Receiver<PageResult<U>>, InternalError>;
}

#[async_trait]
impl<T: Uid> Pager for T {
    async fn get_page_by_prefix<U: Clone + Send + fmt::Debug +'static> 
        (tree: &Tree, buffer_size: usize, key: Option<Self>, page: Option<u32>, page_size: Option<u32>, default_page_size: u32, 
            f: Box<dyn Fn((sled::IVec, sled::IVec)) -> PageResult<U> + Send>)
            -> Result<Receiver<PageResult<U>>, InternalError>
            {
        let (mut tx, rx) = mpsc::channel::<PageResult<U>>(buffer_size);
        let tree1 = tree.clone();
        
        match process_uid(key, |_, uid_bytes| Ok(uid_bytes.clone())) {
            Ok((uid, uid_bytes)) => { 
                debug!("Received Uid: {:?}", uid.clone());
                tokio::spawn(async move {
                    let actual_page_size = page_size.unwrap_or(default_page_size) as usize;
                    let to_skip = (page.unwrap_or(0) as usize) * actual_page_size;
                    let page_data: Vec<PageResult<U>> = tree1.scan_prefix(uid_bytes).
                        skip(to_skip).
                        take(actual_page_size).
                        map(|next_v| match next_v {
                            Ok(k) => f(k),
                            Err(e) => PageResult::KeyError(e.into())
                        }).
                        collect();

                    debug!("Page: {:?}", page_data.clone());

                    for pd in page_data {
                        let bts = pd.clone();
                        match tx.send(pd).await {
                            Ok(()) => debug!("Sending key: {:?}", bts),// (),
                            Err(e) => error!("Value message transfer failed with: {}", e)
                        }
                    };
                });

                Ok(rx)
            },
            Err(e) => Err(e)
        }
    }
}

pub async fn get_page_by_prefix_u64<U: Send + Clone + fmt::Debug + 'static>
    (tree: &Tree, buffer_size: usize, key: KeyU64, page: Option<u32>, page_size: Option<u32>, default_page_size: u32, 
        f: Box<dyn Fn((sled::IVec, sled::IVec)) -> PageResult<U> + Send>)
         -> Result<Receiver<PageResult<U>>, InternalError> {
    Pager::get_page_by_prefix(tree, buffer_size, Some(key), page, page_size, default_page_size, f).await
}

pub async fn get_page_by_prefix_str<U: Send + Clone + fmt::Debug + 'static>
    (tree: &Tree, buffer_size: usize, key: Option<KeyString>, page: Option<u32>, page_size: Option<u32>, default_page_size: u32, 
        f: Box<dyn Fn((sled::IVec, sled::IVec)) -> PageResult<U> + Send>)
         -> Result<Receiver<PageResult<U>>, InternalError> {
    Pager::get_page_by_prefix(tree, buffer_size, key, page, page_size, default_page_size, f).await
}

pub async fn get_page_by_prefix_bytes<U: Send + Clone + fmt::Debug + 'static>
    (tree: &Tree, buffer_size: usize, key: Option<KeyVec>, page: Option<u32>, page_size: Option<u32>, default_page_size: u32, 
        f: Box<dyn Fn((sled::IVec, sled::IVec)) -> PageResult<U> + Send>)
         -> Result<Receiver<PageResult<U>>, InternalError> {
    Pager::get_page_by_prefix(tree, buffer_size, key, page, page_size, default_page_size, f).await
}


#[derive(Debug)]
pub enum DeleteResult<T: Uid> {
    Success (T),
    NotFound (T),
    Error (InternalError)
}

pub fn delete<T: Uid> (tree: &Tree, key: Option<T>) -> DeleteResult<T> {
    match process_uid(key, |_, uid_bytes| { 
        let v = tree.remove(uid_bytes)?;
        Ok(v) 
    }) {
        Ok((uid, Some(_))) => DeleteResult::Success(uid),
        Ok((uid, None)) => DeleteResult::NotFound(uid),
        Err(e) => DeleteResult::Error(e)
    }
}

#[derive(Debug)]
pub enum AddResult<T: Uid> {
    Success (T),
    Exists (T),
    Error (InternalError)
}

pub fn add<T: Uid, U: ProstMessage> (tree: &TransactionalTree, key: Option<T>, value: Option<U>) -> AddResult<T> {
    let res = || -> Result<AddResult<T>, InternalError> {
        let val = unwrap_field(value, "value")?;
        let value_bytes = val.to_bytes()?;

        let check_and_insert = move |uid: &T, uid_bytes: &Vec<u8>| -> Result<_, InternalError> {
            //let contains = tree.contains_key(uid_bytes)?;
            let get_result: Result<_, ::sled::ConflictableTransactionError<Option<IVec>>> = tree.get(uid_bytes.clone()).map_err(|e| e.into());
            let contains = get_result?;
            match contains { 
                Some(_) => Ok(AddResult::<T>::Exists(uid.clone())),
                None => {
                    let existing_result: Result<_, ::sled::ConflictableTransactionError<Option<IVec>>> = tree.insert(uid_bytes.clone(), value_bytes).map_err(|e| e.into());
                    let existing = existing_result?; 
                    if existing.is_some() {
                        error!("Unexpected override of the value in store: '{}'", uid); 
                    }
                    Ok(AddResult::<T>::Success(uid.clone()))
                }
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