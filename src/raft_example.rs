#![warn(unused_imports)]
#[macro_use]

extern crate sled;
//extern crate raft;

extern crate log;

extern crate tonic;

//use crate::raft_cluster::raft_cluster_main;
use sled::{Config as SledConfig, Result};
// use raft::{
//     Config,
//     storage::MemStorage,
//     raw_node::RawNode,
// };

//mod raft_cluster;

// use raft_cluster;

fn basic() -> Result<()> {
    let config = SledConfig::new().temporary(true);

    let db = config.open()?;

    let k = b"k".to_vec();
    let v1 = b"v1".to_vec();
    let v2 = b"v2".to_vec();

    // set and get
    db.insert(k.clone(), v1.clone())?;
    assert_eq!(db.get(&k).unwrap().unwrap(), (v1.clone()));

    // compare and swap
    match db.compare_and_swap(k.clone(), Some(&v1.clone()), Some(v2.clone()))? {
        Ok(()) => println!("it worked!"),
        Err(sled::CompareAndSwapError { current: cur, proposed: _ }) => {
            println!("the actual current value is {:?}", cur)
        }
    }

    // scan forward
    let mut iter = db.range(k.as_slice()..);
    let (k1, v1) = iter.next().unwrap().unwrap();
    assert_eq!(v1, v2.clone());
    assert_eq!(k1, k.clone());
    assert_eq!(iter.next(), None);

    // deletion
    db.remove(&k)?;

    Ok(())
}

fn merge_operator() -> Result<()> {
    fn concatenate_merge(
        _key: &[u8],              // the key being merged
        old_value: Option<&[u8]>, // the previous value, if one existed
        merged_bytes: &[u8],      // the new bytes being merged in
    ) -> Option<Vec<u8>> {
        // set the new value, return None to delete
        let mut ret = old_value.map_or_else(|| vec![], |ov| ov.to_vec());

        ret.extend_from_slice(merged_bytes);

        Some(ret)
    }

    let config = SledConfig::new().temporary(true);

    let db = config.open()?;
    db.set_merge_operator(concatenate_merge);

    let k = b"k".to_vec();

    db.insert(k.clone(), vec![0])?;
    db.merge(k.clone(), vec![1])?;
    db.merge(k.clone(), vec![2])?;
    assert_eq!(db.get(&*k).unwrap().unwrap(), (vec![0, 1, 2]));

    // sets replace previously merged data,
    // bypassing the merge function.
    db.insert(k.clone(), vec![3])?;
    assert_eq!(db.get(&*k).unwrap().unwrap(), (vec![3]));

    // merges on non-present values will add them
    db.remove(&*k)?;
    db.merge(k.clone(), vec![4])?;
    assert_eq!(db.get(&*k).unwrap().unwrap(), (vec![4]));

    Ok(())
}

// fn init_raft() -> Result<()> {
//     // Select some defaults, then change what we need.
//     let config = Config {
//         id: 1,
//         ..Default::default()
//     };
//     // ... Make any configuration changes.
//     // After, make sure it's valid!
//     config.validate().unwrap();
//     // We'll use the built-in `MemStorage`, but you will likely want your own.
//     // Finally, create our Raft node!
//     let storage = MemStorage::new_with_conf_state((vec![1], vec![]));
//     let mut node = RawNode::new(&config, storage).unwrap();
//     // We will coax it into being the lead of a single node cluster for exploration.
//     node.raft.become_candidate();
//     node.raft.become_leader();
    
//     Ok(())
// }

fn main() -> Result<()> {
    // let t: Vec<i32> = (1..6).collect();
    // println!("{:?}", t);
    //raft_cluster_main();

    basic()?;
    merge_operator()?;
    // init_raft()?;

    Ok (())
}
