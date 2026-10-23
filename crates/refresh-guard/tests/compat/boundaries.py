#!/usr/bin/env python3
"""Author-reviewed boundary cases: expectations do not come from the port."""
import json
from pathlib import Path


def main():
    cases = []

    def add(name, before, after, gate='admit', klass='live', reason=None, comment=None, changed=None, stock=None):
        expected = dict(admission=gate, **{'class': klass})
        if reason is not None:
            expected['reason'] = reason
        if changed is not None:
            expected['changed_fns'] = changed
        item = dict(id=name, before=before if isinstance(before, dict) else {'lib.rs': before},
                    after=after if isinstance(after, dict) else {'lib.rs': after}, expected=expected)
        if comment:
            item['comment'] = comment
        if stock:
            item['stock'] = stock
        cases.append(item)

    n = 'fn n() -> u8 { 1 }'
    helper = "fn fixture_factory() -> u8 { 1 } fn n() -> u8 { 1 }"
    add('literal', n, n.replace('1', '2'), changed=['::n'], stock='valid')
    add('identical', n, n, changed=[])
    add('comment', n, '// leading comment\n' + n, changed=[])
    add('formatting', n, 'fn n ( ) -> u8\n{ 1 }', changed=[])
    add('crlf', n, n + '\r\n', changed=[])
    add('unicode-identifier', 'fn café() -> u8 { 1 }', 'fn café() -> u8 { 2 }', changed=['::café'])
    add('parse-incomplete', n, 'fn n( {', 'parse', 'stock', 'parse', stock='invalid')
    add('parse-unclosed-string', n, 'fn n() -> u8 { "', 'parse', 'stock', 'parse')
    add('parse-priority-over-prefix', helper, 'fn fixture_factory() -> u8 { 1 } fn n() { fixture_fa; fn broken( }', 'parse', 'stock', 'parse')
    add('prefix-name', helper, helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { fixture_fa }'),
        'incomplete-expression', stock='invalid')
    add('prefix-call', helper, helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { fixture_fa() }'),
        'incomplete-expression', stock='invalid')
    add('prefix-owner-site', helper + ' fn other() { let fixture_fa = 1; let _ = fixture_fa; }',
        helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { fixture_fa }') + ' fn other() { let fixture_fa = 1; let _ = fixture_fa; }',
        'incomplete-expression', comment='Existing expression in a different function does not exempt the new site.')
    add('prefix-existing-site', 'fn fixture_factory() {} fn n() { fixture_fa; }',
        'fn fixture_factory() {} fn n() { fixture_fa; let x = 2; }')
    add('prefix-new-function-exempt', helper, helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { fixture_fa() }') + ' fn fixture_fa() -> u8 { 2 }')
    add('prefix-current-function-name', 'fn n() {}', 'fn n() { newly(); } fn newly_added() {}', 'incomplete-expression')
    add('exp119-false-reject-local-prefix', helper,
        helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { let fixture_fa = 2; fixture_fa }'),
        'incomplete-expression', comment='Known EXP-119 false rejection: valid local binding is a strict prefix.', stock='valid')
    add('prefix-outranks-tail', helper, helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { fixture_fa; }'), 'incomplete-expression')
    add('tail-empty', n, 'fn n() -> u8 {}', 'missing-return-tail', stock='invalid')
    add('tail-let', n, 'fn n() -> u8 { let x = 1; }', 'missing-return-tail', stock='invalid')
    add('tail-semicolon', n, 'fn n() -> u8 { 1; }', 'missing-return-tail', stock='invalid')
    add('tail-explicit-return', n, 'fn n() -> u8 { return 2; }', stock='valid')
    add('tail-default-unit', 'fn n() {}', 'fn n() { let x = 1; }', stock='valid')
    add('tail-explicit-unit', 'fn n() -> () {}', 'fn n() -> () { let x = 1; }', stock='valid')
    add('tail-existing-violation', 'fn n() -> u8 {}', 'fn n() -> u8 { let x = 1; }')
    add('exp119-false-reject-panic-semicolon', n, 'fn n() -> u8 { panic!(); }', 'missing-return-tail', 'restart', 'macro-body',
        comment='Known EXP-119 false rejection: diverging macro with semicolon is valid Rust.', stock='valid')
    add('panic-tail-no-semicolon', n, 'fn n() -> u8 { panic!() }', klass='restart', reason='macro-body', stock='valid')
    add('exp119-false-accept-wrong-tail-type', n, 'fn n() -> u8 { "wrong" }',
        comment='Known EXP-119 false acceptance: syntax-only gate does not type-check tails.', stock='invalid')
    add('tail-method', 'struct A; impl A { fn n() -> u8 { 1 } }',
        'struct A; impl A { fn n() -> u8 {} }', 'missing-return-tail', stock='invalid')
    add('tail-outranks-bare', helper, helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { let x = fixture_factory; }'), 'missing-return-tail')
    add('bare-function-value', helper, helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { fixture_factory }'), 'bare-function-reference', stock='invalid')
    add('exp119-false-reject-function-pointer', helper, helper.replace('fn n() -> u8 { 1 }',
        'fn n() -> u8 { let pointer_value: fn() -> u8 = fixture_factory; pointer_value() }'), 'bare-function-reference',
        comment='Known EXP-119 false rejection: first-class fn values are valid.', stock='valid')
    add('bare-argument', helper, helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { use_fn(fixture_factory) }') +
        ' fn use_fn(callback: fn() -> u8) -> u8 { callback() }', 'bare-function-reference', stock='valid')
    add('bare-reference-existing', 'fn h() {} fn n() { let f = h; }', 'fn h() {} fn n() { let f = h; let x = 1; }')
    add('unknown-call', n, 'fn n() -> u8 { missing() }', 'unknown-call', stock='invalid')
    add('unknown-call-existing', 'fn n() -> u8 { missing() }', 'fn n() -> u8 { missing() + 1 }')
    add('known-call', helper, helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { fixture_factory() }'), stock='valid')
    add('new-local-call', n, 'fn n() -> u8 { helper() } fn helper() -> u8 { 2 }', changed=['::helper', '::n'], stock='valid')
    add('qualified-call-unchecked', n, 'fn n() -> u8 { module::missing() }', stock='invalid')
    add('method-call-unchecked', n, 'fn n() -> u8 { 1_u8.missing() }', stock='invalid')
    add('unknown-closure-call-false-reject', n, 'fn n() -> u8 { let callback = || 2_u8; callback() }',
        'unknown-call', 'restart', 'macro-body', comment='No binding resolution: a valid local closure call is treated as unknown.', stock='valid')
    add('unknown-outranks-removal', n + ' fn removed() {}', 'fn n() -> u8 { missing() }', 'unknown-call', 'restart', 'macro-body')
    add('removed-function', n + ' fn gone() {}', n, 'removed-function', 'restart', 'macro-body', stock='valid')
    add('removed-inline-module-function', 'mod a { fn n() {} }', 'mod a {}', 'removed-function', 'restart', 'macro-body')
    add('removed-method-not-admission-fn', 'struct A; impl A { fn n() {} }', 'struct A; impl A {}', klass='restart', reason='layout')
    add('signature-change', n, 'fn n() -> u16 { 1 }', klass='restart', reason='macro-body', stock='valid')
    add('visibility-change', n, 'pub fn n() -> u8 { 1 }', klass='restart', reason='macro-body')
    add('struct-field', 'struct A { x: u8 }', 'struct A { x: u16 }', klass='restart', reason='layout')
    add('enum-variant', 'enum A { X }', 'enum A { X, Y }', klass='restart', reason='layout')
    add('derive-change', 'struct A;', '#[derive(Clone)] struct A;', klass='restart', reason='layout')
    add('const-change', 'const X: u8 = 1;', 'const X: u8 = 2;', klass='restart', reason='layout')
    add('static-change', 'static X: u8 = 1;', 'static X: u8 = 2;', klass='restart', reason='layout')
    add('import-change', 'use std::fmt::Debug; ' + n, 'use std::fmt::Display; ' + n,
        klass='restart', reason='layout', comment='Actual prototype includes use items in structure, so changed imports restart.')
    add('impl-header', 'struct A; impl A { fn n() {} }', 'struct A; impl A { pub fn n() {} }', klass='restart', reason='layout')
    add('file-doc', '//! first\n' + n, '//! changed\n' + n, klass='restart', reason='layout')
    add('function-doc', '/// first\n' + n, '/// changed\n' + n, klass='restart', reason='macro-body')
    add('ordinary-method-body', 'struct A; impl A { fn n() -> u8 { 1 } }',
        'struct A; impl A { fn n() -> u8 { 2 } }', changed=[])
    add('method-macro-body', 'struct A; impl A { fn n() { log!(1); } }',
        'struct A; impl A { fn n() { log!(2); } }', klass='restart', reason='layout')
    add('method-closure-body', 'struct A; impl A { fn n() { let x = || 1; } }',
        'struct A; impl A { fn n() { let x = || 2; } }', klass='restart', reason='layout')
    add('method-local-item', 'struct A; impl A { fn n() { struct B; } }',
        'struct A; impl A { fn n() { struct C; } }', klass='restart', reason='layout')
    add('async-post-await-string', 'async fn n() { tick().await; "before"; }',
        'async fn n() { tick().await; "after"; }', klass='restart', reason='async')
    add('async-before-await-string', 'async fn n() { let s = "before"; tick().await; }',
        'async fn n() { let s = "after"; tick().await; }', klass='restart', reason='async')
    add('async-local-across-await', 'async fn n() { tick().await; }',
        'async fn n() { let value = [7_u64; 32]; tick().await; let _ = value; }', klass='restart', reason='async')
    add('async-comment-only', 'async fn n() { tick().await; }', 'async fn n() { /* comment */ tick().await; }', changed=[])
    add('async-block-string', 'fn n() { let f = async { "before" }; }', 'fn n() { let f = async { "after" }; }',
        klass='restart', reason='macro-body')
    add('async-move-block-string', 'fn n() { let f = async move { "before" }; }', 'fn n() { let f = async move { "after" }; }',
        klass='restart', reason='macro-body')
    add('async-method-string', 'struct A; impl A { async fn n() { "before"; } }',
        'struct A; impl A { async fn n() { "after"; } }', klass='restart', reason='layout')
    add('async-block-in-method', 'struct A; impl A { fn n() { let f = async { "before" }; } }',
        'struct A; impl A { fn n() { let f = async { "after" }; } }', klass='restart', reason='layout')
    add('async-trait-body', 'trait A { async fn n() { "before"; } }',
        'trait A { async fn n() { "after"; } }', klass='restart', reason='layout')
    add('new-async-function', n, n + ' async fn other() {}', klass='restart', reason='new-complex-fn')
    add('main-body', 'fn main() { let x = 1; }', 'fn main() { let x = 2; }', klass='restart', reason='macro-body')
    add('attributed-body', '#[inline(never)] ' + n, '#[inline(never)] ' + n.replace('1', '2'), klass='restart', reason='macro-body')
    add('doc-only-body', '/// docs\n' + n, '/// docs\n' + n.replace('1', '2'), changed=['::n'])
    add('macro-tokens', 'fn n() { sqlx::query!("SELECT 1"); }', 'fn n() { sqlx::query!("SELECT 2"); }', klass='restart', reason='macro-body')
    add('macro-unchanged-nearby-body', 'fn n() { log!("same"); let x = 1; }',
        'fn n() { log!("same"); let x = 2; }', changed=['::n'])
    add('nested-item-change', 'fn n() { fn inner() -> u8 { 1 } }', 'fn n() { fn inner() -> u8 { 2 } }', klass='restart', reason='macro-body')
    add('closure-body-change', 'fn n() { let callback = || 1; }', 'fn n() { let callback = || 2; }', klass='restart', reason='macro-body')
    add('new-simple-fn', n, n + ' fn added() {}', changed=['::added'])
    add('new-const-fn', n, n + ' const fn added() -> u8 { 1 }', changed=['::added'])
    add('new-generic-fn', n, n + ' fn added<T>() {}', klass='restart', reason='new-complex-fn')
    add('new-unsafe-fn', n, n + ' unsafe fn added() {}', klass='restart', reason='new-complex-fn')
    add('new-extern-fn', n, n + ' extern "C" fn added() {}', klass='restart', reason='new-complex-fn')
    add('new-attributed-fn', n, n + ' #[inline] fn added() {}', klass='restart', reason='new-complex-fn')
    add('new-doc-fn', n, n + '\n/// docs\nfn added() {}', changed=['::added'])
    add('new-fn-with-closure', n, n + ' fn added() { let f = || 1; }', klass='restart', reason='new-complex-fn')
    add('new-fn-with-macro', n, n + ' fn added() { log!("hello"); }', klass='restart', reason='new-complex-fn')
    add('existing-generic-body', 'fn n<T>() -> u8 { 1 }', 'fn n<T>() -> u8 { 2 }', changed=['::n'])
    add('existing-unsafe-body', 'unsafe fn n() -> u8 { 1 }', 'unsafe fn n() -> u8 { 2 }', changed=['::n'])
    add('existing-extern-body', 'extern "C" fn n() -> u8 { 1 }', 'extern "C" fn n() -> u8 { 2 }', changed=['::n'])
    add('module-qualified-functions', 'mod a { fn n() -> u8 { 1 } } mod b { fn n() -> u8 { 2 } }',
        'mod a { fn n() -> u8 { 3 } } mod b { fn n() -> u8 { 2 } }', changed=['a::n'])
    for path in ['Cargo.toml', 'Cargo.lock', 'build.rs', '.cargo/config.toml', '.cargo/config']:
        old_config = 'fn main() { let value = 1; }' if path == 'build.rs' else '# before'
        new_config = 'fn main() { let value = 2; }' if path == 'build.rs' else '# after'
        add('manifest-' + path.replace('/', '-'), {path: old_config}, {path: new_config}, klass='stock', reason='manifest')
        add('manifest-add-' + path.replace('/', '-'), {}, {path: new_config}, klass='stock', reason='manifest')
        add('manifest-remove-' + path.replace('/', '-'), {path: old_config}, {}, klass='stock', reason='manifest')
    add('identical-manifest', {'Cargo.toml': '[package]'}, {'Cargo.toml': '[package]'}, changed=[])
    add('file-add', {}, {'new.rs': 'fn n() {}'}, klass='restart', reason='file-set')
    add('file-remove', {'gone.rs': 'struct A;'}, {}, klass='restart', reason='file-set')
    add('manifest-before-file-set', {'Cargo.toml': 'before'}, {'Cargo.toml': 'after', 'new.rs': ''}, klass='stock', reason='manifest')
    add('manifest-before-parse', {'Cargo.toml': 'before', 'lib.rs': n}, {'Cargo.toml': 'after', 'lib.rs': 'fn n( {'},
        'parse', 'stock', 'manifest')
    add('layout-before-async-global', {'a.rs': 'async fn n() { 1; }', 'z.rs': 'struct A;'},
        {'a.rs': 'async fn n() { 2; }', 'z.rs': 'struct A { x: u8 }'}, klass='restart', reason='layout')
    add('async-before-macro-global', {'a.rs': 'fn n() { log!(1); }', 'z.rs': 'async fn n() { 1; }'},
        {'a.rs': 'fn n() { log!(2); }', 'z.rs': 'async fn n() { 2; }'}, klass='restart', reason='async')
    add('parse-before-prefix-global', {'a.rs': helper, 'z.rs': n},
        {'a.rs': helper.replace('fn n() -> u8 { 1 }', 'fn n() -> u8 { fixture_fa }'), 'z.rs': 'fn n( {'},
        'parse', 'stock', 'parse')
    add('empty-scope', {}, {}, changed=[])
    root = Path(__file__).resolve().parent.parent / 'cases'
    (root / 'boundaries.json').write_text(json.dumps(cases, indent=2) + '\n')
    print(f'{len(cases)} boundary cases')


if __name__ == '__main__':
    main()
