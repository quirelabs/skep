//! The README's table of tools, checked against the tools.
//!
//! Documentation drifts silently: a tool added without a row is a capability
//! nobody knows about, and a row without a tool is a promise. The first had
//! already happened to share and unshare, which is why this exists.

/// Every tool the server registers, read out of its own source rather than
/// listed again here, so adding one cannot be forgotten in two places.
fn registered() -> Vec<String> {
    let source = include_str!("../src/main.rs");
    let mut found: Vec<String> = source
        .match_indices("name = \"skep_")
        .filter_map(|(at, _)| {
            let rest = &source[at + "name = \"".len()..];
            rest.find('"').map(|end| rest[..end].to_string())
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

fn documented() -> Vec<String> {
    let readme = include_str!("../../../README.md");
    let mut found: Vec<String> = readme
        .match_indices("`skep_")
        .filter_map(|(at, _)| {
            let rest = &readme[at + 1..];
            rest.find('`').map(|end| rest[..end].to_string())
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

#[test]
fn every_tool_is_in_the_readme_and_every_row_is_a_tool() {
    let (registered, documented) = (registered(), documented());

    assert!(
        !registered.is_empty(),
        "no tools were found in the server's source; this test has stopped reading it"
    );
    let undocumented: Vec<_> = registered
        .iter()
        .filter(|tool| !documented.contains(tool))
        .collect();
    assert!(
        undocumented.is_empty(),
        "tools nobody is told about: {undocumented:?}"
    );
    let imaginary: Vec<_> = documented
        .iter()
        .filter(|tool| !registered.contains(tool))
        .collect();
    assert!(
        imaginary.is_empty(),
        "rows for tools that do not exist: {imaginary:?}"
    );
}
