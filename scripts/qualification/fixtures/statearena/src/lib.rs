//! Experimental single-writer, restart-persistent, schema-tagged arena.
pub use statearena_derive::Persist;
use serde::{Deserialize, Serialize};
use std::{fs::{File, OpenOptions}, os::fd::AsRawFd, path::Path};
pub type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Field { pub name: String, pub offset: usize, pub schema: Schema }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Schema { pub name: String, pub size: usize, pub fields: Vec<Field>, pub element: Option<Box<Schema>>, pub hash: u64 }
fn hash(bytes: &[u8]) -> u64 { bytes.iter().fold(0xcbf29ce484222325, |h,b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)) }
impl Schema {
    pub fn record(name: &str, size: usize, fields: Vec<Field>) -> Self { Self::make(name,size,fields,None) }
    fn make(name: &str, size: usize, fields: Vec<Field>, element: Option<Box<Schema>>) -> Self {
        let mut s = Self { name:name.into(), size, fields, element, hash:0 };
        s.hash = hash(&serde_json::to_vec(&s).unwrap()); s
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Value { Unsigned(u64), Signed(i64), Float(f64), Bool(bool), Text(String), Record(Vec<(String,Value)>), List(Vec<Value>), Optional(Option<Box<Value>>) }
pub trait Persist: Sized { fn schema() -> Schema; fn encode(&self) -> Value; fn decode(value:&Value, old:&Schema, log:&mut Vec<String>) -> Result<Self>; }
pub fn record_fields<'a>(value:&'a Value, old:&Schema, name:&str, new:&Schema, log:&mut Vec<String>) -> Result<&'a [(String,Value)]> {
    if old.name != name { return Err(format!("record type changed: {} -> {name}",old.name)); }
    let Value::Record(fields) = value else { return Err("expected record".into()); };
    if fields.len()!=old.fields.len() || old.fields.iter().any(|f| fields.iter().filter(|(n,_)|n==&f.name).count()!=1) { return Err("record/schema field mismatch".into()); }
    if old.hash != new.hash {
        log.push(format!("migrate {name}: {:016x} -> {:016x}",old.hash,new.hash));
        for f in &old.fields { if !new.fields.iter().any(|n|n.name==f.name) {log.push(format!("drop {name}.{}",f.name));} }
    }
    Ok(fields)
}
pub fn field<T: Persist + Default>(name:&str, fields:&[(String,Value)], old:&Schema, log:&mut Vec<String>) -> Result<T> {
    match old.fields.iter().find(|f|f.name==name) {
        Some(f) => T::decode(&fields.iter().find(|(n,_)|n==name).ok_or("missing stored field")?.1,&f.schema,log).map_err(|e|format!("{}.{name}: {e}",old.name)),
        None => { log.push(format!("default {}.{name}",old.name)); Ok(T::default()) }
    }
}
fn numeric(old:&Schema, target:&str, log:&mut Vec<String>) -> Result<()> {
    if old.name==target {return Ok(());}
    let width = |n:&str| -> Option<(char,u16)> { let mut it=n.chars(); let c=it.next()?; if !['u','i','f'].contains(&c) {return None;} Some((c,it.as_str().parse().ok()?)) };
    let a=width(&old.name).ok_or("unsupported numeric source")?;
    let b=width(target).ok_or("unsupported numeric target")?;
    if (a.0==b.0 && a.1<b.1) || (a.0=='u' && b.0=='i' && a.1<b.1) {
        log.push(format!("widen {} -> {target}",old.name)); Ok(())
    } else { Err(format!("refuse numeric conversion {} -> {target}",old.name)) }
}
macro_rules! uint { ($($t:ty),*) => { $(impl Persist for $t {
    fn schema()->Schema {Schema::record(stringify!($t),size_of::<Self>(),vec![])}
    fn encode(&self)->Value {Value::Unsigned(*self as u64)}
    fn decode(v:&Value,s:&Schema,l:&mut Vec<String>)->Result<Self> {numeric(s,stringify!($t),l)?; match v {Value::Unsigned(n)=><$t>::try_from(*n).map_err(|_|"integer overflow".into()), _=>Err("expected unsigned".into())}}
})* }; }
uint!(u8,u16,u32,u64);
macro_rules! sint { ($($t:ty),*) => { $(impl Persist for $t {
    fn schema()->Schema {Schema::record(stringify!($t),size_of::<Self>(),vec![])}
    fn encode(&self)->Value {Value::Signed(*self as i64)}
    fn decode(v:&Value,s:&Schema,l:&mut Vec<String>)->Result<Self> {numeric(s,stringify!($t),l)?; match v {Value::Signed(n)=><$t>::try_from(*n).map_err(|_|"integer overflow".into()), Value::Unsigned(n)=><$t>::try_from(*n).map_err(|_|"integer overflow".into()), _=>Err("expected integer".into())}}
})* }; }
sint!(i8,i16,i32,i64);
macro_rules! float { ($($t:ty),*) => { $(impl Persist for $t {
    fn schema()->Schema {Schema::record(stringify!($t),size_of::<Self>(),vec![])}
    fn encode(&self)->Value {Value::Float(*self as f64)}
    fn decode(v:&Value,s:&Schema,l:&mut Vec<String>)->Result<Self> {numeric(s,stringify!($t),l)?; match v {Value::Float(n) if n.is_finite()=>Ok(*n as Self), _=>Err("expected finite float".into())}}
})* }; }
float!(f32,f64);
impl Persist for bool {
    fn schema()->Schema {Schema::record("bool",size_of::<Self>(),vec![])}
    fn encode(&self)->Value {Value::Bool(*self)}
    fn decode(v:&Value,s:&Schema,_:&mut Vec<String>)->Result<Self> {match v {Value::Bool(n) if s.name=="bool"=>Ok(*n),_=>Err("expected bool".into())}}
}
impl Persist for String {
    fn schema()->Schema {Schema::record("String",size_of::<Self>(),vec![])}
    fn encode(&self)->Value {Value::Text(self.clone())}
    fn decode(v:&Value,s:&Schema,_:&mut Vec<String>)->Result<Self> {match v {Value::Text(n) if s.name=="String"=>Ok(n.clone()),_=>Err("expected String".into())}}
}
impl<T:Persist> Persist for Vec<T> {
    fn schema()->Schema {Schema::make("Vec",size_of::<Self>(),vec![],Some(Box::new(T::schema())))}
    fn encode(&self)->Value {Value::List(self.iter().map(Persist::encode).collect())}
    fn decode(v:&Value,s:&Schema,l:&mut Vec<String>)->Result<Self> {let Value::List(v)=v else{return Err("expected list".into())}; if s.name!="Vec" {return Err("expected Vec schema".into())}; let e=s.element.as_deref().ok_or("missing element schema")?; v.iter().map(|v|T::decode(v,e,l)).collect()}
}
impl<T:Persist> Persist for Option<T> {
    fn schema()->Schema {Schema::make("Option",size_of::<Self>(),vec![],Some(Box::new(T::schema())))}
    fn encode(&self)->Value {Value::Optional(self.as_ref().map(|v|Box::new(v.encode())))}
    fn decode(v:&Value,s:&Schema,l:&mut Vec<String>)->Result<Self> {let Value::Optional(v)=v else{return Err("expected optional".into())}; if s.name!="Option" {return Err("expected Option schema".into())}; let e=s.element.as_deref().ok_or("missing element schema")?; v.as_ref().map(|v|T::decode(v,e,l)).transpose()}
}
#[derive(Serialize,Deserialize)]
struct Document { version:u32, schema:Schema, value:Value }
// Fixed two-slot file. Header per slot: magic[8], generation u64, len u64, checksum u64.
const SLOT:usize=4*1024*1024;
const HEADER:usize=32;
const MAGIC:&[u8;8]=b"STARENA1";
pub struct Arena { ptr:*mut u8, _file:File, generation:u64, active:Option<usize> }
// Accessed only by exclusive Bevy systems; ownership may move to its worker thread.
unsafe impl Send for Arena {}
unsafe impl Sync for Arena {}
impl Arena {
    pub fn open(path:impl AsRef<Path>)->Result<Self> {
        let file=OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(|e|e.to_string())?;
        if unsafe {libc::flock(file.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)} != 0 {return Err("arena already has a writer".into());}
        let len=file.metadata().map_err(|e|e.to_string())?.len();
        if len==0 { file.set_len((2*SLOT) as u64).map_err(|e|e.to_string())?; }
        else if len!=(2*SLOT) as u64 {return Err("bad arena file length".into());}
        let ptr=unsafe {libc::mmap(std::ptr::null_mut(),2*SLOT,libc::PROT_READ|libc::PROT_WRITE,libc::MAP_SHARED,file.as_raw_fd(),0)};
        if ptr==libc::MAP_FAILED {return Err(std::io::Error::last_os_error().to_string());}
        let mut a=Self{ptr:ptr.cast(),_file:file,generation:0,active:None};
        for i in 0..2 {if let Some((g,_))=a.slot(i) {if g>a.generation {a.generation=g;a.active=Some(i);}}}
        if a.active.is_none() && (0..2).any(|i|a.bytes()[i*SLOT..i*SLOT+HEADER].iter().any(|b|*b!=0)) {return Err("no valid committed checkpoint (corrupt/incomplete)".into());}
        Ok(a)
    }
    fn bytes(&self)->&[u8] {unsafe {std::slice::from_raw_parts(self.ptr,2*SLOT)}}
    fn slot(&self,i:usize)->Option<(u64,&[u8])> {
        let b=&self.bytes()[i*SLOT..(i+1)*SLOT]; if &b[..8]!=MAGIC {return None;}
        let n=|p|u64::from_le_bytes(b[p..p+8].try_into().unwrap());
        let len=usize::try_from(n(16)).ok()?; if len>SLOT-HEADER || n(8)==0 {return None;}
        let payload=&b[HEADER..HEADER+len]; if hash(payload)!=n(24) {return None;}
        Some((n(8),payload))
    }
    pub fn restore<T:Persist>(&self)->Result<Option<(T,Vec<String>)>> {
        let Some(i)=self.active else {return Ok(None);};
        let (_,payload)=self.slot(i).ok_or("checkpoint checksum changed")?;
        let doc:Document=serde_json::from_slice(payload).map_err(|e|e.to_string())?;
        if doc.version!=1 {return Err("unsupported document version".into());}
        let mut log=Vec::new(); let value=T::decode(&doc.value,&doc.schema,&mut log)?;
        log.sort();log.dedup(); Ok(Some((value,log)))
    }
    pub fn save<T:Persist>(&mut self,value:&T)->Result<usize> {
        let value=value.encode();
        fn finite(v:&Value)->bool {match v {Value::Float(n)=>n.is_finite(),Value::Record(f)=>f.iter().all(|(_,v)|finite(v)),Value::List(v)=>v.iter().all(finite),Value::Optional(v)=>v.as_ref().is_none_or(|v|finite(v)),_=>true}}
        if !finite(&value) {return Err("non-finite floats are unsupported by JSON checkpoints".into());}
        let data=serde_json::to_vec(&Document{version:1,schema:T::schema(),value}).map_err(|e|e.to_string())?;
        if data.len()>SLOT-HEADER {return Err("checkpoint exceeds arena slot".into());}
        let i=self.active.map_or(0,|i|1-i); let generation=self.generation.checked_add(1).ok_or("generation overflow")?;
        let b=unsafe {std::slice::from_raw_parts_mut(self.ptr.add(i*SLOT),SLOT)};
        b[..8].fill(0); std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
        b[HEADER..HEADER+data.len()].copy_from_slice(&data);
        b[8..16].copy_from_slice(&generation.to_le_bytes());
        b[16..24].copy_from_slice(&(data.len() as u64).to_le_bytes());
        b[24..32].copy_from_slice(&hash(&data).to_le_bytes());
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst); b[..8].copy_from_slice(MAGIC);
        self.generation=generation;self.active=Some(i);Ok(data.len())
    }
}
impl Drop for Arena {fn drop(&mut self) {unsafe {libc::munmap(self.ptr.cast(),2*SLOT);}}}

// Web codecs: opaque leaves have explicit stable names; records remain typed.
macro_rules! serde_leaf { ($ty:ty, $name:expr) => { impl Persist for $ty {
 fn schema()->Schema { Schema::record($name,size_of::<Self>(),vec![]) }
 fn encode(&self)->Value { Value::Text(serde_json::to_string(self).unwrap()) }
 fn decode(v:&Value,s:&Schema,_:&mut Vec<String>)->Result<Self> {
  if s.name!=$name {return Err("leaf schema mismatch".into())}
  match v { Value::Text(x)=>serde_json::from_str(x).map_err(|e|e.to_string()), _=>Err("expected text".into()) }
 }
} }; }
serde_leaf!(uuid::Uuid,"Uuid/serde-v1");
serde_leaf!(chrono::DateTime<chrono::Utc>,"DateTimeUtc/serde-v1");
