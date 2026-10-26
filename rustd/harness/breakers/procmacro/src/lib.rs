//! What a proc macro gives that source text cannot.

/// The name of a number, matched against constants a macro wrote as negative literals.
pub fn name(number: i8) -> &'static str {
    const NUMBERS: (i8, i16, f32) = macros::negative_numbers!();
    const MINUS_ONE: i8 = NUMBERS.0;
    match number {
        MINUS_ONE => "minus one",
        0 => "zero",
        _ => "other",
    }
}

pub fn numbers() -> (i8, i16, f32) {
    macros::negative_numbers!()
}

/// What the records of `sized` hold, and how large one is.
pub fn records() -> (Vec<u32>, usize) {
    let records = sized::records();
    let held = records.iter().map(|record| match record {
        sized::Record::Short(word) => *word,
        sized::Record::Long(body) => body.words.iter().sum(),
    });
    (held.collect(), std::mem::size_of::<sized::Record>())
}

pub const ATTRIBUTE_LINE: u32 = line!() + 1;
#[macros::locations]
#[inline(never)]
pub fn attribute_locations() -> (u32, u32, u32, u32) {
    (generated_line, generated_location, line!(), std::panic::Location::caller().line())
}
pub const BODY_LINE: u32 = line!() - 2;

pub const DIRECT_LINE: u32 = line!() + 2;
pub fn direct_locations() -> (u32, u32) {
    (line!(), std::panic::Location::caller().line())
}
