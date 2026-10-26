use super::*;

#[test]
fn identifier_origins_follow_replaced_macro_ranges() {
    let at = |end| TextRange::new(TextSize::from(end - 1), TextSize::from(end));
    let mut outer = SpanMap::empty();
    outer.push_ident_origin(2.into(), IdentOrigin::Metavar);
    outer.push_ident_origin(5.into(), IdentOrigin::DocComment);
    outer.push_ident_origin(10.into(), IdentOrigin::Alone);
    outer.finish();
    let mut inner = SpanMap::empty();
    inner.push_ident_origin(1.into(), IdentOrigin::DocComment);
    inner.finish();
    outer.merge(TextRange::new(3.into(), 7.into()), 2.into(), &inner);
    assert_eq!(outer.ident_origin(at(2)), IdentOrigin::Metavar);
    assert_eq!(outer.ident_origin(at(4)), IdentOrigin::DocComment);
    assert_eq!(outer.ident_origin(at(5)), IdentOrigin::Source);
    assert_eq!(outer.ident_origin(at(8)), IdentOrigin::Alone);
    assert_eq!(outer.ident_origin(at(10)), IdentOrigin::Source);
}
