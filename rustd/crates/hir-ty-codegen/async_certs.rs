//! Compact EXP-109/111 proofs. Only baseline compilation and emitted definitions
//! call `descriptor`; there is deliberately no whole-program certificate query.
use super::*;
static DESCRIPTORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static MIR_INPUTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub(super) fn counts() -> (usize, usize) {
    use std::sync::atomic::Ordering::Relaxed;
    (DESCRIPTORS.load(Relaxed), MIR_INPUTS.load(Relaxed))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Descriptor {
    kind: u8,
    owner_file: String,
    owner_path: String,
    owner_line: u32,
    frame: u128,
    entry: u128,
    resumes: Vec<(u32, u128)>,
}
#[derive(Clone)]
pub(super) struct Row {
    pub symbol: String,
    pub old: Option<Descriptor>,
    pub new: Option<Descriptor>,
}
pub(super) fn eligible(db: &dyn HirDatabase, instance: &Instance) -> bool {
    match instance.def {
        InstanceDef::Coroutine(_) | InstanceDef::CoroutineByMove(_) => true,
        InstanceDef::Closure(id) => {
            let owner = InternedClosureId::from_id(id).loc(db).owner;
            match owner {
                InferBodyId::DefWithBodyId(DefWithBodyId::FunctionId(func)) => {
                    // Protect closures declared in async owners, including macro
                    // wrappers such as tokio::main (a sync fn containing async).
                    // Ordinary sync-only closures remain under the strict source
                    // guard and need no async proof cache or MIR fingerprint.
                    FunctionSignature::of(db, func).is_async()
                        || super::async_blocks(db, func).as_ref().is_none_or(|blocks| !blocks.is_empty())
                }
                _ => true, // Unknown/initializer ownership stays conservative.
            }
        }
        _ => false,
    }
}
/// A code serial alone cannot detect a changed frame whose emitted poll code
/// happens to be identical. Track monomorphized MIR as well, without retaining it.
#[salsa::tracked(returns(copy))]
pub(super) fn mir_serial<'db>(db: &'db dyn HirDatabase, instance: Instance, env: StoredParamEnvAndCrate) -> u128 {
    MIR_INPUTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let body = super::instance_body(db, &instance, env.clone());
    // A named const/type can keep its MIR ID while its evaluated layout changes.
    // Depend on actual layouts as codegen does, including captures/frame receiver.
    let layouts: Vec<_> = body.as_ref().ok().into_iter().flat_map(|body| body.locals.iter())
        .map(|(_, local)| format!("{:?}", db.layout_of_ty(local.ty.clone(), env.clone())))
        .collect();
    print_of(&format!("{body:?};layouts={layouts:?}"))
}

/// Retain only the compact proof in Salsa. Rebuilding a native baseline must
/// not recompute certificates for every unchanged coroutine/closure again.
#[salsa::tracked(returns(ref))]
fn certificate<'db>(db: &'db dyn HirDatabase, instance: Instance, env: StoredParamEnvAndCrate) -> Option<Descriptor> {
    if !eligible(db, &instance) { return None; }
    DESCRIPTORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    compact(&super::frame_certificate109(db, instance, env)?)
}
pub(super) fn descriptor(db: &dyn HirDatabase, instance: &Instance, env: &StoredParamEnvAndCrate) -> Option<Descriptor> {
    let mut result = certificate(db, instance.clone(), env.clone()).clone()?;
    // Source positions are coverage metadata, not protected semantic input.
    // Keeping this outside `certificate` avoids invalidating every following
    // function's proof when a line is inserted above it.
    let owner = match instance.def {
        InstanceDef::Coroutine(id) | InstanceDef::CoroutineByMove(id) => InternedCoroutineId::from_id(id).loc(db).owner,
        InstanceDef::Closure(id) => InternedClosureId::from_id(id).loc(db).owner,
        _ => return None,
    };
    if let InferBodyId::DefWithBodyId(DefWithBodyId::FunctionId(func)) = owner {
        result.owner_line = declaration_location(db, func).as_ref().map_or(0, |(_, line, _)| *line);
    }
    Some(result)
}

fn compact(raw: &str) -> Option<Descriptor> {
    let fields: Vec<_> = raw.split(';').collect();
    let field = |prefix: &str| {
        let mut found = fields.iter().filter_map(|p| p.strip_prefix(prefix));
        let value = found.next()?;
        found.next().is_none().then_some(value)
    };
    let owner_file = field("owner_file=")?.to_owned();
    let owner_path = field("owner_path=")?.to_owned();
    let owner_line = field("owner_line=")?.parse().ok()?;
    let resume = field("resume111=")?;
    let entry = field("entry111=")?;
    let hash = |s: &str| (s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())).then(|| u128::from_str_radix(s, 16).ok()).flatten();
    let (kind, entry, resumes) = if let Some(hash_value) = resume.strip_prefix("closure-v1:") {
        if entry != "none" { return None; }
        (2, 0, vec![(0, hash(hash_value)?)])
    } else {
        let mut parts = resume.split(',');
        if parts.next()? != "v1" { return None; }
        let resumes: Vec<_> = parts.map(|p| {
            let (state, value) = p.split_once(':')?;
            Some((state.parse::<u32>().ok()?, hash(value)?))
        }).collect::<Option<_>>()?;
        // Resume numbering must describe every physical suspended state exactly.
        let states: Vec<_> = fields.iter().filter_map(|p| p.strip_prefix("suspend#")).map(|p| p.split_once(':')?.0.parse::<u32>().ok()).collect::<Option<_>>()?;
        if !states.iter().copied().eq(resumes.iter().map(|r| r.0)) || states.windows(2).any(|s| s[0] >= s[1]) { return None; }
        (1, hash(entry)?, resumes)
    };
    let frame = fields.into_iter().filter(|p| !p.starts_with("entry111=") && !p.starts_with("resume111=") && !p.starts_with("owner_line=")).collect::<Vec<_>>().join(";");
    Some(Descriptor { kind, owner_file, owner_path, owner_line, frame: print_of(&frame), entry, resumes })
}
fn blob(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u32).to_le_bytes()); out.extend_from_slice(value);
}
fn proof(out: &mut Vec<u8>, value: &Option<Descriptor>) {
    let Some(d) = value else { out.push(0); return; };
    out.push(d.kind);
    blob(out, d.owner_file.as_bytes()); blob(out, d.owner_path.as_bytes()); out.extend_from_slice(&d.owner_line.to_le_bytes());
    out.extend_from_slice(&d.frame.to_le_bytes()); out.extend_from_slice(&d.entry.to_le_bytes());
    out.extend_from_slice(&(d.resumes.len() as u32).to_le_bytes());
    for (state, hash) in &d.resumes { out.extend_from_slice(&state.to_le_bytes()); out.extend_from_slice(&hash.to_le_bytes()); }
}
pub(super) fn encode(rows: &[Row]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(rows.len() as u32).to_le_bytes());
    for r in rows { blob(&mut body, r.symbol.as_bytes()); proof(&mut body, &r.old); proof(&mut body, &r.new); }
    let mut out = b"ACRT0001".to_vec(); blob(&mut out, &body); out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_preserves_frame_and_resume_but_excludes_entry() {
        let raw = "frame-v2 owner=f;owner_path=app::f;owner_file=src/lib.rs;owner_line=1;layout=8;entry111=00000000000000000000000000000001;resume111=v1,3:00000000000000000000000000000002;suspend#3:kept=[];";
        let a = compact(raw).unwrap();
        let b = compact(&raw.replace("entry111=00000000000000000000000000000001", "entry111=00000000000000000000000000000009")).unwrap();
        assert_eq!(a.frame, b.frame); assert_eq!(a.resumes, b.resumes); assert_ne!(a.entry, b.entry);
        assert_ne!(a.frame, compact(&raw.replace("layout=8", "layout=16")).unwrap().frame);
        assert_ne!(a.resumes, compact(&raw.replace(",3:00000000000000000000000000000002", ",3:00000000000000000000000000000003")).unwrap().resumes);
        assert!(compact(&raw.replace("suspend#3", "suspend#4")).is_none());
        assert!(compact(&raw.replace("resume111=v1", "resume111=v2")).is_none());
    }
}
