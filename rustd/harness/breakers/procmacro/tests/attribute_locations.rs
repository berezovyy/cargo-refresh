//! Source spans for synthesized attribute tokens, forwarded tokens and direct calls.
#[test]
fn generated_and_written_tokens_keep_distinct_source_locations() {
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = lines.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        seen.lock().unwrap().push(info.location().unwrap().line());
    }));
    let actual = procmacro::attribute_locations();
    std::panic::set_hook(previous);
    assert_eq!(actual, (procmacro::ATTRIBUTE_LINE, procmacro::ATTRIBUTE_LINE,
        procmacro::BODY_LINE, procmacro::BODY_LINE));
    assert_eq!(*lines.lock().unwrap(), [procmacro::ATTRIBUTE_LINE]);
    assert_eq!(procmacro::direct_locations(), (procmacro::DIRECT_LINE, procmacro::DIRECT_LINE));
}
