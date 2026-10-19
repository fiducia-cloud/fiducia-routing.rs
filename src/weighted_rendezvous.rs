//! Opt-in v1 logical-partition + weighted rendezvous placement.
//! Legacy fnv1a(key) % shard_count routing remains untouched. Do not switch a
//! live Raft fleet without epoch cutover/reconciliation and golden vectors.
use std::collections::BTreeSet;
pub const PARTITIONS:u32=65_536;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Lifecycle { Joining, Active, Draining, Removed }
#[derive(Clone,Debug)]
pub struct Node<'a>{pub id:&'a str,pub failure_domain:&'a str,pub weight:u32,pub lifecycle:Lifecycle}
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct Placement<'a>{pub partition:u32,pub primary:&'a str,pub replicas:Vec<&'a str>}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum PlacementError{InvalidMap,NotEnoughFailureDomains}
fn hash(bytes:&[u8])->u64 {
 let mut h=0xcbf29ce484222325u64;
 for b in bytes{h^=u64::from(*b);h=h.wrapping_mul(0x100000001b3);}
 h
}
pub fn logical_partition(domain:&str)->Result<u32,PlacementError>{
 if domain.is_empty()||domain.len()>1024 {return Err(PlacementError::InvalidMap);}
 Ok((hash(domain.as_bytes())%u64::from(PARTITIONS)) as u32)
}
fn node_hash(partition:u32,node:&str)->u64{
 let mut s=partition.to_be_bytes().to_vec();
 s.extend_from_slice(&(node.len() as u32).to_be_bytes());
 s.extend_from_slice(node.as_bytes());hash(&s)
}
/// Score = -ln(U)/weight, smallest wins. Finite weights avoid NaN,
/// zero-capacity nodes and floating-point provider-specific random seeds.
pub fn place<'a>(partition:u32,nodes:&'a[Node<'a>],replicas:usize)->Result<Placement<'a>,PlacementError>{
 if partition>=PARTITIONS||nodes.is_empty()||nodes.len()>1024||replicas>16{
  return Err(PlacementError::InvalidMap);
 }
 let mut seen=BTreeSet::new();
 for n in nodes {
  if n.id.is_empty()||n.id.len()>128||n.failure_domain.is_empty()||n.weight==0
     ||n.weight>1_000_000||!seen.insert(n.id){
   return Err(PlacementError::InvalidMap);
  }
 }
 let mut ranked=nodes.iter().filter(|n|n.lifecycle==Lifecycle::Active)
  .map(|n|{
    let u=(node_hash(partition,n.id) as f64+1.0)/(u64::MAX as f64+2.0);
    (-u.ln()/f64::from(n.weight),n)
  }).collect::<Vec<_>>();
 ranked.sort_by(|a,b|a.0.total_cmp(&b.0).then(a.1.id.cmp(b.1.id)));
 let mut domains=BTreeSet::new();
 let mut winners=Vec::new();
 for (_,n) in ranked {
  if domains.insert(n.failure_domain) {winners.push(n.id);}
  if winners.len()==replicas+1 {break;}
 }
 if winners.len()!=replicas+1 {return Err(PlacementError::NotEnoughFailureDomains);}
 Ok(Placement{partition,primary:winners[0],replicas:winners[1..].to_vec()})
}
#[cfg(test)]mod tests{
 use super::*;
 fn a()->Vec<Node<'static>>{vec![Node{id:"a",failure_domain:"d1",weight:1,lifecycle:Lifecycle::Active},Node{id:"b",failure_domain:"d2",weight:1,lifecycle:Lifecycle::Active}]}
 #[test]fn reordering_nodes_does_not_change_rank(){
  for p in 0..1000 {let mut n=a();let x=place(p,&n,1).unwrap();let original=x.primary.to_owned();let original_replicas=x.replicas.iter().map(|x|x.to_string()).collect::<Vec<_>>();n.reverse();let reversed=place(p,&n,1).unwrap();assert_eq!(original,reversed.primary);assert_eq!(original_replicas,reversed.replicas);}
 }
 #[test]fn removal_only_moves_partitions_previously_owned(){
  let mut n=a();n.push(Node{id:"c",failure_domain:"d3",weight:1,lifecycle:Lifecycle::Active});
  for p in 0..1000 {
   let before=place(p,&n,0).unwrap().primary;
   let remaining=a();
   let after=place(p,&remaining,0).unwrap().primary;
   if before!="c"{assert_eq!(before,after);}
  }
 }
 #[test]fn draining_and_failure_domain_exclusion(){
  let mut n=a();n[1].lifecycle=Lifecycle::Draining;
  assert_eq!(place(8,&n,1),Err(PlacementError::NotEnoughFailureDomains));
  n.push(Node{id:"c",failure_domain:"d1",weight:9,lifecycle:Lifecycle::Active});
  assert_eq!(place(8,&n,1),Err(PlacementError::NotEnoughFailureDomains));
 }
 #[test]fn invalid_epoch_inputs_do_not_succeed(){
  assert_eq!(logical_partition(""),Err(PlacementError::InvalidMap));
  assert_eq!(place(PARTITIONS,&a(),0),Err(PlacementError::InvalidMap));
 }
}
