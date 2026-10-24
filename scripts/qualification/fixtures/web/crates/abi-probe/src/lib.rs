#![allow(non_camel_case_types,dead_code)]
use std::{collections::HashMap,num::{NonZeroU64,NonZeroU8},sync::Arc,convert::Infallible};
use axum::{Router,body::Body,http::{Request,Response}};
pub type abi_map=HashMap<u32,Arc<str>>;
pub type abi_map_routes=HashMap<u32,Router>;
pub type abi_router=Router<()>;
pub type abi_box=tower::util::BoxCloneSyncService<Request<Body>,Response<Body>,Infallible>;
pub type abi_niche=Option<NonZeroU64>;
pub type abi_nested_niche=Option<Option<NonZeroU8>>;
pub type abi_project=domain::Project;
pub type abi_ticket=domain::Ticket;
pub type abi_stamp=domain::VersionStamp;
pub type abi_status=domain::RecordStatus;
pub type abi_simd=std::arch::x86_64::__m128i;
pub unsafe fn abi_sse_load(p:*const abi_simd)->abi_simd {unsafe{std::arch::x86_64::_mm_loadu_si128(p)}}
pub fn abi_sse_eq(a:abi_simd,b:abi_simd)->abi_simd {unsafe{std::arch::x86_64::_mm_cmpeq_epi8(a,b)}}
pub fn abi_sse_mask(a:abi_simd)->i32 {unsafe{std::arch::x86_64::_mm_movemask_epi8(a)}}
pub fn abi_map_insert(m:&mut abi_map,k:u32,v:Arc<str>)->Option<Arc<str>> {m.insert(k,v)}
pub fn abi_router_clone(r:&Router)->Router {r.clone()}
pub fn abi_box_clone(b:&abi_box)->abi_box {b.clone()}
pub fn abi_domain_exchange(p:domain::Project)->domain::Project {p}
pub fn abi_niche_exchange(p:abi_niche)->abi_niche {p}
