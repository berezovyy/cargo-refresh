#![feature(rustc_private)]
extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_hir;
extern crate rustc_abi;
extern crate rustc_index;

use rustc_driver::{Callbacks, Compilation};
use rustc_interface::interface;
use rustc_middle::ty::{self, Ty, TyCtxt, Instance, TypingEnv, TypeVisitableExt};
use rustc_hir::def::DefKind;

use std::collections::HashMap;

fn q(s: impl AsRef<str>) -> String {
    let mut o=String::from("\"");
    for c in s.as_ref().chars() { match c { '"'=>o.push_str("\\\""), '\\'=>o.push_str("\\\\"), '\n'=>o.push_str("\\n"), '\r'=>o.push_str("\\r"), '\t'=>o.push_str("\\t"), c if c<' '=>o.push_str(&format!("\\u{:04x}",c as u32)), c=>o.push(c) } }
    o.push('"');o
}
fn obj(fields: &[(&str,String)]) -> String {format!("{{{}}}",fields.iter().map(|(k,v)|format!("{}:{}",q(k),v)).collect::<Vec<_>>().join(","))}
fn arr(v: Vec<String>) -> String {format!("[{}]",v.join(","))}
fn emit(fields: &[(&str,String)]) {println!("{}",obj(fields));}
struct Facts;
fn fn_fact<'tcx>(tcx:TyCtxt<'tcx>, label:&str, inst:Instance<'tcx>) {
    let env=TypingEnv::fully_monomorphized();
    match tcx.fn_abi_of_instance(env.as_query_input((inst,ty::List::empty()))) {
        Ok(a)=>emit(&[("kind",q("fn")),("name",q(label)),("instance",q(inst.to_string())),("conv",q(format!("{:?}",a.conv))),("args",arr(a.args.iter().map(|a|obj(&[("ty",q(a.layout.ty.to_string())),("mode",q(format!("{:?}",a.mode))),("size",a.layout.size.bytes().to_string())])).collect())),("ret",obj(&[("ty",q(a.ret.layout.ty.to_string())),("mode",q(format!("{:?}",a.ret.mode))),("size",a.ret.layout.size.bytes().to_string())])),("can_unwind",a.can_unwind.to_string())]),
        Err(e)=>emit(&[("kind",q("fn_error")),("name",q(label)),("error",q(format!("{e:?}")))])
    }
}
fn visit<'tcx>(tcx:TyCtxt<'tcx>, t:Ty<'tcx>, depth:usize, seen:&mut HashMap<Ty<'tcx>,usize>) {
    if depth>32 || seen.len()>12000 || seen.get(&t).is_some_and(|d| *d<=depth) || t.has_non_region_param() {return}
    let first=seen.insert(t,depth).is_none();
    let env=TypingEnv::fully_monomorphized();
    
    let Ok(l)=tcx.layout_of(env.as_query_input(t)) else {return};
    let mut children=Vec::new();
    let mut fields=Vec::new();
    let mut variants=Vec::new();
    let encoding=match &l.variants {
        rustc_abi::Variants::Empty => obj(&[("kind",q("empty"))]),
        rustc_abi::Variants::Single{index} => obj(&[("kind",q("single")),("index",index.index().to_string())]),
        rustc_abi::Variants::Multiple{tag,tag_encoding,tag_field,..} => obj(&[("kind",q("multiple")),("tag",q(format!("{tag:?}"))),("encoding",q(format!("{tag_encoding:?}"))),("tag_field",tag_field.index().to_string()),("tag_offset",l.fields.offset(tag_field.index()).bytes().to_string())])
    };
    if let ty::Adt(adt,args)=t.kind() {
        children.extend(args.types());
        for (vi,v) in adt.variants().iter_enumerated() {
            let vl=l.for_variant(&ty::layout::LayoutCx::new(tcx,env),vi);
            let mut vf=Vec::new();
            for (i,f) in v.fields.iter().enumerate() {
                let ft=tcx.normalize_erasing_regions(env,f.ty(tcx,args)); children.push(ft);
                vf.push(obj(&[("name",q(f.name.to_string())),("ty",q(ft.to_string())),("offset",vl.fields.offset(i).bytes().to_string())]));
            }
            variants.push(obj(&[("discriminant",if adt.is_enum(){q(adt.discriminant_for_variant(tcx,vi).val.to_string())}else{"null".into()}),("index",vi.index().to_string()),("name",q(v.name.to_string())),("fields",arr(vf))]));
        }
    } else if let ty::Tuple(ts)=t.kind() {
        for (i,ft) in ts.iter().enumerate(){children.push(ft);fields.push(obj(&[("index",i.to_string()),("ty",q(ft.to_string())),("offset",l.fields.offset(i).bytes().to_string())]));}
    } else if let ty::RawPtr(ft,_) | ty::Ref(_,ft,_) | ty::Array(ft,_) | ty::Slice(ft) | ty::Pat(ft,_)=t.kind() {children.push(*ft);}
    if first {emit(&[("kind",q("type")),("name",q(t.to_string())),("size",l.size.bytes().to_string()),("align",l.align.bytes().to_string()),("backend_repr",q(format!("{:?}",l.backend_repr))),("encoding",encoding),("niche",q(format!("{:?}",l.largest_niche))),("fields",arr(fields)),("variants",arr(variants)),("type_id",q(format!("{:032x}",tcx.type_id_hash(t).as_u128())))]);
    if let ty::Dynamic(preds,_) = t.kind() {
        if let Some(p)=preds.principal() {
            let tr=p.skip_binder().with_self_ty(tcx,t);
            let entries=tcx.vtable_entries(tr);
            emit(&[("kind",q("vtable")),("name",q(t.to_string())),("entries",arr(entries.iter().enumerate().map(|(i,e)|obj(&[("slot",i.to_string()),("entry",q(format!("{e:?}")))])).collect()))]);
            for e in entries {if let ty::VtblEntry::Method(inst)=e {fn_fact(tcx,&format!("vtable:{t}:{inst}"),*inst);}}
        }
    }
    }
    for c in children {visit(tcx,c,depth+1,seen);}
}
impl Callbacks for Facts {
 fn after_analysis<'tcx>(&mut self,_:&interface::Compiler,tcx:TyCtxt<'tcx>)->Compilation {
    emit(&[("kind",q("manifest")),("schema", "1".into()),("panic_strategy",q(format!("{:?}",tcx.sess.panic_strategy()))),("opt_level",q(format!("{:?}",tcx.sess.opts.optimize))),("rustc",q(option_env!("ABI_FACTS_RUSTC").unwrap_or("unknown"))),("target",q(tcx.sess.opts.target_triple.to_string())),("crate",q(tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).to_string()))]);
    let mut roots=Vec::new();
    for id in tcx.iter_local_def_id() {
        let d=id.to_def_id();
        if !matches!(tcx.def_kind(d),DefKind::Fn | DefKind::TyAlias | DefKind::Struct | DefKind::Enum) {continue}
        if !tcx.item_name(d).as_str().starts_with("abi_") {continue}
        match tcx.def_kind(d) {
            DefKind::TyAlias | DefKind::Struct | DefKind::Enum if tcx.generics_of(d).count()==0 => {
                let t=tcx.normalize_erasing_regions(TypingEnv::fully_monomorphized(),tcx.type_of(d).instantiate_identity()); roots.push(t);
                emit(&[("kind",q("root")),("name",q(tcx.def_path_str(d))),("ty",q(t.to_string()))]);
            },
            DefKind::Fn if tcx.generics_of(d).count()==0=>{
                fn_fact(tcx,&tcx.def_path_str(d),Instance::mono(tcx,d));
                let s=tcx.fn_sig(d).instantiate_identity().skip_binder();
                roots.extend(s.inputs().iter().copied()); roots.push(s.output());
                // Resolve concrete direct callees from the selected function's MIR.
                let body=tcx.optimized_mir(d);
                for block in body.basic_blocks.iter() {
                    if let rustc_middle::mir::TerminatorKind::Call{func,..}=&block.terminator().kind {
                        let ft=func.ty(&body.local_decls,tcx);
                        if let ty::FnDef(callee,args)=ft.kind() {
                            if let Ok(Some(inst))=Instance::try_resolve(tcx,TypingEnv::fully_monomorphized(),*callee,args.skip_binder()) {
                                fn_fact(tcx,&format!("callee:{}:{}",tcx.def_path_str(d),inst),inst);
                            }
                        }
                    }
                }
            },_=>{}
        }
    }
    let mut seen=HashMap::new();for t in roots {visit(tcx,t,0,&mut seen);}
    for (item,d) in tcx.lang_items().iter() {
        let mut row=vec![("kind",q("lang_item")),("name",q(format!("{item:?}"))),("path",q(tcx.def_path_str(d)))];
        if matches!(tcx.def_kind(d),DefKind::Fn) && tcx.generics_of(d).count()==0 {row.push(("symbol",q(tcx.symbol_name(Instance::mono(tcx,d)).name)));}
        emit(&row);
    }
    Compilation::Stop
 }
}
fn main(){let args:Vec<String>=std::env::args().collect();
#[cfg(abi_nightly)] rustc_driver::compiler_entrypoint(&args,&mut Facts);
#[cfg(not(abi_nightly))] rustc_driver::run_compiler(&args,&mut Facts);}
