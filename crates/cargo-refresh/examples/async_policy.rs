//! Manual EXP-109/111 harness bridge. Decisions use the production proof reader.
use anyhow::{Context, Result};
use cargo_refresh::manifest::AsyncRuntime;
use refresh_proto::async_certs::Descriptor;
fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
fn describe(d: &Descriptor) -> String {
    let resume = if d.kind == 2 {
        format!("closure-v1:{}", hex(&d.resumes[0].1))
    } else {
        format!(
            "v1{}",
            d.resumes
                .iter()
                .map(|(state, hash)| format!(",{state}:{}", hex(hash)))
                .collect::<String>()
        )
    };
    format!(
        "frame-v2 owner=proof;owner_path={};owner_file={};frame={};entry111={};resume111={};{}",
        d.owner_path,
        d.owner_file,
        hex(&d.frame),
        if d.kind == 2 {
            "none".into()
        } else {
            hex(&d.entry)
        },
        resume,
        if d.kind == 1 {
            d.resumes
                .iter()
                .map(|(state, _)| format!("suspend#{state}:;"))
                .collect::<String>()
        } else {
            String::new()
        }
    )
}
fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(
        a.first()
            .context("usage: async_policy MANIFEST [OWNER_FILE OWNER_NAME]")?,
    )?;
    let m = refresh_proto::manifest::parse(&bytes)?;
    let owners = if a.len() == 3 {
        vec![refresh_guard::ChangedFn {
            path: a[1].clone().into(),
            name: a[2].clone(),
        }]
    } else {
        vec![]
    };
    let mut runtime = AsyncRuntime::default();
    runtime.observe(&bytes)?;
    let refusal = runtime.decide(&owners).err();
    let rows: Vec<_> = m
        .async_certificates
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|r| {
            serde_json::json!([
                r.symbol,
                r.old.as_ref().map(describe),
                r.new.as_ref().map(describe)
            ])
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({"action":if refusal.is_some() {"restart-state"} else {"live"}, "refusal":refusal,"rows":rows,"binary":m.async_certificates.is_some(),"generation":m.generation})
    );
    Ok(())
}
