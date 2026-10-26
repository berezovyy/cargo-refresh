//! EXP-111: suspension-delimited semantic certificates, from monomorphized MIR.
//! Frame-held locals are anchored to their slots; other locals are alpha-renamed by first use.
use super::*;
use std::collections::VecDeque;

fn operand(op: &mut Operand) {
    op.span = None;
}
fn edges(term: &mut TerminatorKind, mut f: impl FnMut(&mut BasicBlockId)) {
    match term {
        TerminatorKind::Goto { target } => f(target),
        TerminatorKind::SwitchInt { targets, .. } => {
            // SwitchTargets keeps its order: branch values remain attached to their edges.
            for target in &mut targets.targets {
                f(target);
            }
        }
        TerminatorKind::Drop { target, unwind, .. }
        | TerminatorKind::DropAndReplace { target, unwind, .. } => {
            f(target);
            unwind.iter_mut().for_each(f);
        }
        TerminatorKind::Call { target, cleanup, .. } => {
            target.iter_mut().for_each(&mut f);
            cleanup.iter_mut().for_each(f);
        }
        TerminatorKind::Assert { target, cleanup, .. } => {
            f(target);
            cleanup.iter_mut().for_each(f);
        }
        // Resume is an explicitly named boundary, not an ordinary CFG edge.
        TerminatorKind::Yield { drop, .. } => drop.iter_mut().for_each(f),
        TerminatorKind::FalseEdge { real_target, imaginary_target } => {
            f(real_target);
            f(imaginary_target);
        }
        TerminatorKind::FalseUnwind { real_target, unwind } => {
            f(real_target);
            unwind.iter_mut().for_each(f);
        }
        _ => (),
    }
}
struct Locals<'a, 'db> {
    body: &'a MirBody<'db>,
    db: &'db dyn HirDatabase,
    env: crate::ParamEnvAndCrate<'db>,
    failed: bool,
    ids: FxHashMap<LocalId, LocalId>,
    types: Vec<String>,
}
impl<'db> Locals<'_, 'db> {
    fn local(&mut self, local: &mut LocalId) {
        let n = self.ids.len() as u32;
        *local = *self.ids.entry(*local).or_insert_with(|| {
            self.types.push(format!("{:?}", self.body.locals[*local].ty));
            LocalId::from_raw(la_arena::RawIdx::from_u32(n))
        });
    }
    fn place(&mut self, p: &mut StoredPlace) {
        self.local(&mut p.local);
        let projection: Vec<_> = p
            .projection
            .as_slice()
            .iter()
            .copied()
            .map(|elem| {
                elem.map(|mut local| {
                    self.local(&mut local);
                    local
                })
            })
            .collect();
        p.projection = super::super::Projection::new_from_slice(&projection).store();
    }
    fn operand(&mut self, op: &mut Operand) {
        operand(op);
        // MIR may name a constant whose initializer changed without changing its ID.
        // Hash the same evaluated allocation that poll codegen consumes.
        if let OperandKind::Constant { konst, .. } = &op.kind {
            if let ConstKind::Unevaluated(UnevaluatedConst { def, args }) = konst.as_ref().kind() {
                let result = match def.0 {
                    GeneralConstId::ConstId(id) => {
                        let infcx = DbInterner::new_with(self.db, self.env.krate)
                            .infer_ctxt()
                            .build(TypingMode::PostAnalysis);
                        let (id, args) = lookup_impl_const(&infcx, self.env.param_env, id, args);
                        self.db.const_eval(id, args, Some(self.env))
                    }
                    GeneralConstId::StaticId(id) => self.db.const_eval_static(id),
                    GeneralConstId::AnonConstId(id) => {
                        self.db.anon_const_eval(id, args, Some(self.env))
                    }
                };
                match result {
                    Ok(allocation) => {
                        op.kind = OperandKind::Allocation { allocation: allocation.store() }
                    }
                    Err(_) => self.failed = true,
                }
            }
        }
        match &mut op.kind {
            OperandKind::Move(p) | OperandKind::Copy(p) => self.place(p),
            _ => (),
        }
    }
    fn asm(&mut self, a: &mut super::super::InlineAsm) {
        for op in &mut a.operands {
            match op {
                super::super::AsmOperand::Reg { input, output, .. } => {
                    input.iter_mut().for_each(|op| self.operand(op));
                    output.iter_mut().for_each(|p| self.place(p));
                }
                super::super::AsmOperand::Const(op) | super::super::AsmOperand::Sym(op) => {
                    self.operand(op)
                }
                _ => (),
            }
        }
        a.taken.iter_mut().for_each(|p| self.place(p));
    }
    fn stmt(&mut self, s: &mut StatementKind) {
        match s {
            StatementKind::Assign(p, r) => {
                self.place(p);
                match r {
                    Rvalue::Use(o)
                    | Rvalue::Repeat(o, _)
                    | Rvalue::Cast(_, o, _)
                    | Rvalue::UnaryOp(_, o) => self.operand(o),
                    Rvalue::CheckedBinaryOp(_, a, b) => {
                        self.operand(a);
                        self.operand(b);
                    }
                    Rvalue::Aggregate(_, ops) => ops.iter_mut().for_each(|op| self.operand(op)),
                    Rvalue::Ref(_, p)
                    | Rvalue::Len(p)
                    | Rvalue::Discriminant(p)
                    | Rvalue::CopyForDeref(p) => self.place(p),
                    _ => (),
                }
            }
            StatementKind::FakeRead(p) | StatementKind::Deinit(p) => self.place(p),
            StatementKind::StorageLive(l) | StatementKind::StorageDead(l) => self.local(l),
            StatementKind::InlineAsm(a) => self.asm(a),
            _ => (),
        }
    }
    fn term(&mut self, t: &mut TerminatorKind) {
        match t {
            TerminatorKind::SwitchInt { discr, .. } => self.operand(discr),
            TerminatorKind::Drop { place, .. } => self.place(place),
            TerminatorKind::DropAndReplace { place, value, .. } => {
                self.place(place);
                self.operand(value);
            }
            TerminatorKind::Call { func, args, destination, .. } => {
                self.operand(func);
                args.iter_mut().for_each(|o| self.operand(o));
                self.place(destination);
            }
            TerminatorKind::Assert { cond, .. } => self.operand(cond),
            TerminatorKind::Yield { value, resume_arg, .. } => {
                self.operand(value);
                self.place(resume_arg);
            }
            _ => (),
        }
    }
}
fn segment<'db>(
    db: &'db dyn HirDatabase,
    env: crate::ParamEnvAndCrate<'db>,
    body: &MirBody<'db>,
    start: BasicBlockId,
    resumes: &[BasicBlockId],
    held: &[LocalId],
) -> Option<u128> {
    let mut locals =
        Locals { body, db, env, failed: false, ids: FxHashMap::default(), types: Vec::new() };
    // Fixed names for return, arguments and every physical frame slot.
    for mut local in std::iter::once(return_slot())
        .chain(body.param_locals.iter().copied())
        .chain(held.iter().copied())
    {
        locals.local(&mut local);
    }
    if let Some(c) = body.coroutine {
        for mut local in [c.poll, c.context] {
            locals.local(&mut local);
        }
    }
    let mut ids = FxHashMap::default();
    ids.insert(start, 0u32);
    let mut queue = VecDeque::from([start]);
    let mut normalized = String::new();
    while let Some(id) = queue.pop_front() {
        let bb = &body.basic_blocks[id];
        normalized.push_str(&format!("block{}:cleanup={};", ids[&id], bb.is_cleanup));
        for stmt in &bb.statements {
            // These have no runtime code in this backend. Preserve actual Drop,
            // Deinit and FakeRead (indexed FakeRead may check bounds).
            if matches!(
                stmt.kind,
                StatementKind::StorageLive(_) | StatementKind::StorageDead(_) | StatementKind::Nop
            ) {
                continue;
            }
            let mut stmt = stmt.kind.clone();
            locals.stmt(&mut stmt);
            normalized.push_str(&format!("{stmt:?};"));
        }
        let mut term = bb.terminator.as_ref()?.kind.clone();
        locals.term(&mut term);
        edges(&mut term, |next| {
            let n = ids.len() as u32;
            let at = *ids.entry(*next).or_insert_with(|| {
                queue.push_back(*next);
                n
            });
            *next = BasicBlockId::from_raw(la_arena::RawIdx::from_u32(at));
        });
        if let TerminatorKind::Yield { resume, .. } = &mut term {
            let state = resumes.iter().position(|x| x == resume)? as u32;
            *resume = BasicBlockId::from_raw(la_arena::RawIdx::from_u32(state));
        }
        normalized.push_str(&format!("{term:?};"));
    }
    // Drop order of referenced/held locals is part of the fingerprint. Entry-only
    // dead temporaries are excluded, preserving harmless pre-await edits.
    let scope: Vec<_> = body.scope_ends.iter().filter_map(|id| locals.ids.get(id)).collect();
    normalized.push_str(&format!("types={:?};scope_ends={scope:?};", locals.types));
    if locals.failed {
        return None;
    }
    Some(print_of(&normalized))
}

pub(super) fn certificates<'db>(
    db: &'db dyn HirDatabase,
    env: crate::ParamEnvAndCrate<'db>,
    body: &MirBody<'db>,
    held: &super::super::HeldLocals,
) -> Option<String> {
    let resumes: Vec<_> = held.yields.iter().map(|p| p.resume).collect();
    let entry = segment(db, env, body, body.start_block, &resumes, &held.locals)?;
    let mut out = format!("entry111={entry:032x};resume111=v1");
    for (at, &resume) in resumes.iter().enumerate() {
        let hash = segment(db, env, body, resume, &resumes, &held.locals)?;
        out.push_str(&format!(",{}:{hash:032x}", CoroutineFrame::SUSPENDED + at));
    }
    out.push(';');
    Some(out)
}

// Synchronous closures can execute inside a resumed async segment. Capture-layout
// equality alone must not hide a moved effect in their bodies.
pub(super) fn closure<'db>(
    db: &'db dyn HirDatabase,
    env: crate::ParamEnvAndCrate<'db>,
    body: &MirBody<'db>,
) -> Option<String> {
    Some(format!(
        "resume111=closure-v1:{:032x};entry111=none;",
        segment(db, env, body, body.start_block, &[], &[])?
    ))
}
