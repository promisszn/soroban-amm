//! Fails when the AMM pool emits an event the SDK cannot decode (#1389).
//!
//! `decode_amm_event` is documented as covering every event the pool emits, but
//! nothing used to check that: thirteen event families were silently dropped.
//! This test walks every `emit_versioned_event!` site in the pool's source and,
//! for each one, requires that
//!
//! 1. `EVENT_SCHEMAS` has an `amm` entry with the same topic and payload arity,
//!    and
//! 2. `decode_amm_event` returns `Some` for an event of exactly that shape (the
//!    emit site's topic count, the schema's field types).
//!
//! It also checks the converse, so a schema entry cannot outlive the emit site
//! it describes. `events_from_contracts.rs` decodes the payloads the deployed
//! contract really publishes; this test is what catches a *new* emit site that
//! nobody wrote a decoder or a test for. The extraction mirrors
//! `scripts/event_emit_sites.mjs`, which does the same for the webhook-streamer
//! schema.

use soroban_amm_sdk::{
    events::{decode_amm_event, EventSchema, EVENT_SCHEMAS},
    EVENT_SCHEMA_VERSION,
};
use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, IntoVal, Symbol, Val, Vec};

/// The pool's only non-test module. `amm_has_no_other_non_test_modules`
/// fails if that stops being true, so a new module cannot escape the scan.
const AMM_SOURCE: &str = include_str!("../../amm/src/lib.rs");
const AMM_SOURCE_PATH: &str = "contracts/amm/src/lib.rs";

/// One `emit_versioned_event!(env, topics, payload)` call in the pool.
#[derive(Debug)]
struct EmitSite {
    line: usize,
    topic: String,
    topic_count: usize,
    arity: usize,
}

/// Blanks comments (keeping newlines, so line numbers survive) and leaves
/// string literals intact.
fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(' ');
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            let mut depth = 0;
            loop {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                } else {
                    out.push(if bytes[i] == b'\n' { '\n' } else { ' ' });
                    i += 1;
                }
                if depth == 0 || i >= bytes.len() {
                    break;
                }
            }
        } else if bytes[i] == b'"' {
            out.push('"');
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    out.push(bytes[i] as char);
                    i += 1;
                }
                out.push(bytes[i] as char);
                i += 1;
            }
            if i < bytes.len() {
                out.push('"');
                i += 1;
            }
        } else {
            // Copy a whole UTF-8 character.
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Byte index of the bracket closing the one opened at `open`.
fn matching(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut stack: std::vec::Vec<u8> = std::vec::Vec::new();
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'(' => stack.push(b')'),
            b'[' => stack.push(b']'),
            b'{' => stack.push(b'}'),
            c @ (b')' | b']' | b'}') => {
                if stack.pop() != Some(c) {
                    return None;
                }
                if stack.is_empty() {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Splits on commas that are not nested inside brackets or strings.
fn split_top_level(text: &str) -> std::vec::Vec<&str> {
    let bytes = text.as_bytes();
    let mut parts = std::vec::Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(text[start..].trim());
    parts
}

/// The elements of a tuple expression `(a, b,)`, or `None` if `expr` is not a
/// tuple (a bare value, or a parenthesised expression `(x)`).
fn tuple_elements(expr: &str) -> Option<std::vec::Vec<&str>> {
    let expr = expr.trim();
    if !expr.starts_with('(') || matching(expr, 0) != Some(expr.len() - 1) {
        return None;
    }
    let mut parts = split_top_level(&expr[1..expr.len() - 1]);
    let trailing_comma = parts.last() == Some(&"");
    if trailing_comma {
        parts.pop();
    }
    if parts.len() == 1 && !trailing_comma {
        return None;
    }
    Some(parts)
}

/// The string inside the first `"..."` of `text`.
fn quoted(text: &str) -> Option<&str> {
    let start = text.find('"')? + 1;
    let len = text[start..].find('"')?;
    Some(&text[start..start + len])
}

/// Resolves the event name from the first topic expression. Panics on a form
/// it does not recognise, so a new emit style cannot slip past the check.
fn event_name(topic: &str, line: usize) -> String {
    let topic = topic.trim();
    if topic.starts_with("symbol_short!") || topic.starts_with("Symbol::new") {
        if let Some(name) = quoted(topic) {
            return name.to_string();
        }
    }
    panic!(
        "{AMM_SOURCE_PATH}:{line}: cannot resolve the event name from `{topic}`; \
         teach tests/emit_site_coverage.rs this form"
    );
}

fn line_of(src: &str, index: usize) -> usize {
    src[..index].bytes().filter(|&b| b == b'\n').count() + 1
}

/// Every event emit site in the pool's source.
fn amm_emit_sites() -> std::vec::Vec<EmitSite> {
    let src = strip_comments(AMM_SOURCE);

    // The pool emits only through the SDK's versioning macro. A raw publish
    // would bypass the version envelope `decode_amm_event` expects.
    if let Some(idx) = src.find(".publish(") {
        panic!(
            "{AMM_SOURCE_PATH}:{}: raw `.publish(` call; emit through \
             soroban_amm_sdk::emit_versioned_event! so the SDK can decode it",
            line_of(&src, idx)
        );
    }

    let needle = "emit_versioned_event!";
    let mut sites = std::vec::Vec::new();
    let mut from = 0;
    while let Some(rel) = src[from..].find(needle) {
        let at = from + rel;
        let line = line_of(&src, at);
        let open = at
            + src[at..]
                .find('(')
                .unwrap_or_else(|| panic!("{AMM_SOURCE_PATH}:{line}: macro call without `(`"));
        let close = matching(&src, open)
            .unwrap_or_else(|| panic!("{AMM_SOURCE_PATH}:{line}: unbalanced macro call"));
        let args = split_top_level(&src[open + 1..close]);
        assert_eq!(
            args.len(),
            3,
            "{AMM_SOURCE_PATH}:{line}: expected emit_versioned_event!(env, topics, payload)"
        );
        let topics = tuple_elements(args[1])
            .unwrap_or_else(|| panic!("{AMM_SOURCE_PATH}:{line}: topics must be a tuple"));
        let payload = tuple_elements(args[2]).unwrap_or_else(|| {
            panic!("{AMM_SOURCE_PATH}:{line}: payload must be a tuple for decode_amm_event")
        });
        sites.push(EmitSite {
            line,
            topic: event_name(topics[0], line),
            topic_count: topics.len(),
            arity: payload.len(),
        });
        from = close;
    }
    sites
}

fn amm_schemas() -> impl Iterator<Item = &'static EventSchema> {
    EVENT_SCHEMAS.iter().filter(|s| s.contract == "amm")
}

/// A sample value of a schema field type, for building synthetic payloads.
fn sample(env: &Env, ty: &str, topic: &str) -> Val {
    match ty {
        "Address" => Address::generate(env).into_val(env),
        "Option<Address>" => Some(Address::generate(env)).into_val(env),
        "i128" => 1_i128.into_val(env),
        "u64" => 1_u64.into_val(env),
        "u32" => 1_u32.into_val(env),
        "bool" => true.into_val(env),
        "BytesN<32>" => BytesN::from_array(env, &[7; 32]).into_val(env),
        other => panic!("{topic}: no sample value for field type `{other}`; add one to sample()"),
    }
}

#[test]
fn amm_has_no_other_non_test_modules() {
    let src = strip_comments(AMM_SOURCE);
    let lines: std::vec::Vec<&str> = src.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let decl = trimmed
            .strip_prefix("pub(crate) ")
            .or_else(|| trimmed.strip_prefix("pub "))
            .unwrap_or(trimmed);
        if !decl.starts_with("mod ") {
            continue;
        }
        let gated = lines[..i]
            .iter()
            .rev()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .is_some_and(|prev| prev.starts_with("#[cfg(") && prev.contains("test"));
        assert!(
            gated,
            "{AMM_SOURCE_PATH}:{}: `{}` is not test-only; scan it in tests/emit_site_coverage.rs too",
            i + 1,
            trimmed
        );
    }
}

#[test]
fn emit_site_scan_finds_the_pool_events() {
    // Guards the scanner itself: an extraction bug that returned nothing would
    // otherwise make the coverage test pass vacuously.
    let sites = amm_emit_sites();
    for expected in [
        "swap",
        "add_liquidity",
        "emergency_wd",
        "ms_ew",
        "fot_detected",
    ] {
        assert!(
            sites.iter().any(|s| s.topic == expected),
            "scanner did not find the `{expected}` emit site: {sites:?}"
        );
    }
}

#[test]
fn every_amm_emit_site_is_decoded_by_the_sdk() {
    let env = Env::default();
    let mut failures = std::vec::Vec::new();

    for site in amm_emit_sites() {
        let Some(schema) =
            amm_schemas().find(|s| s.topic == site.topic.as_str() && s.fields.len() == site.arity)
        else {
            failures.push(format!(
                "{AMM_SOURCE_PATH}:{}: `{}` with {} payload field(s) has no amm entry in EVENT_SCHEMAS",
                site.line, site.topic, site.arity
            ));
            continue;
        };

        let mut topics: Vec<Val> = Vec::new(&env);
        topics.push_back(Symbol::new(&env, &site.topic).into_val(&env));
        for _ in 1..site.topic_count {
            // Every indexed topic the pool emits after the name is an address
            // (trader, provider, admin, signer, receiver or token).
            topics.push_back(Address::generate(&env).into_val(&env));
        }
        let mut payload: Vec<Val> = Vec::new(&env);
        for (_, ty) in schema.fields {
            payload.push_back(sample(&env, ty, &site.topic));
        }
        let data: Val = (EVENT_SCHEMA_VERSION, payload).into_val(&env);

        if decode_amm_event(&env, topics, data).is_none() {
            failures.push(format!(
                "{AMM_SOURCE_PATH}:{}: decode_amm_event returned None for `{}` \
                 ({} topic(s), {} payload field(s))",
                site.line, site.topic, site.topic_count, site.arity
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "AMM events the SDK does not handle:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn every_amm_schema_matches_an_emit_site() {
    let sites = amm_emit_sites();
    let stale: std::vec::Vec<String> = amm_schemas()
        .filter(|schema| {
            !sites
                .iter()
                .any(|s| s.topic == schema.topic && s.arity == schema.fields.len())
        })
        .map(|schema| format!("`{}` with {} field(s)", schema.topic, schema.fields.len()))
        .collect();
    assert!(
        stale.is_empty(),
        "EVENT_SCHEMAS describes amm events no emit site in {AMM_SOURCE_PATH} produces: {}",
        stale.join(", ")
    );
}
