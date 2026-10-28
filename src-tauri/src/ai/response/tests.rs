use serde_json::{Value, json};

use super::*;

fn candidate(id: i64, text: &str) -> Candidate {
    Candidate {
        id,
        municipality: None,
        neighborhood: None,
        property_type: None,
        comparable_price: None,
        price_spread_percent: None,
        built_area: None,
        lot_area: None,
        bedrooms: None,
        suites: None,
        parking_spaces: None,
        active_listings: 1,
        sources: 1,
        listings: vec![],
        unknown_facts: vec![],
        conflicting_facts: vec![],
        local_score: 50.0,
        data_confidence: "low".to_owned(),
        confidence_reasons: vec![],
        local_components: vec![],
        text: text.to_owned(),
    }
}

fn sent() -> Vec<Candidate> {
    vec![
        candidate(10, "Casa com piscina e quintal amplo, próxima ao parque."),
        candidate(20, "Sobrado novo, 3 quartos, churrasqueira."),
        candidate(30, "Terreno plano de esquina."),
    ]
}

fn item(id: i64, rank: i64) -> Value {
    json!({
        "id": id,
        "rank": rank,
        "explanation": "Boa relação entre preço e área.",
        "positives": [],
        "concerns": ["Preço sem confirmação"],
        "uncertainties": ["Estado de conservação desconhecido"],
    })
}

fn answer(items: Vec<Value>) -> Value {
    json!({ "ranking": items })
}

/// The HTTP body the provider sends for a normal answer carrying `model_text`.
fn envelope(model_text: &str) -> String {
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": [{ "text": model_text }] },
            "finishReason": "STOP"
        }],
        "usageMetadata": { "promptTokenCount": 1 }
    })
    .to_string()
}

fn body_of(answer: &Value) -> String {
    envelope(&answer.to_string())
}

fn ok(answer: &Value) -> Validated {
    parse_answer(&body_of(answer), &sent()).unwrap_or_else(|e| panic!("{e}: {answer}"))
}

fn rejected(answer: &Value) -> String {
    match parse_answer(&body_of(answer), &sent()) {
        Err(AiFailure::OutputRejected(why)) => why,
        other => panic!("expected a rejection, got {other:?} for {answer}"),
    }
}

fn malformed(body: &str) -> AiFailure {
    parse_answer(body, &sent()).expect_err("must not be accepted")
}

// ---- a valid answer ------------------------------------------------------------------------

#[test]
fn a_valid_answer_becomes_notes_ordered_by_the_models_rank() {
    let v = ok(&answer(vec![item(20, 3), item(30, 1), item(10, 2)]));
    let order: Vec<(i64, u32)> = v.notes.iter().map(|n| (n.property_id, n.ai_rank)).collect();
    assert_eq!(order, vec![(30, 1), (10, 2), (20, 3)]);
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    let n = &v.notes[0];
    assert_eq!(n.explanation, "Boa relação entre preço e área.");
    assert_eq!(n.concerns, vec!["Preço sem confirmação".to_owned()]);
    assert_eq!(
        n.uncertainties,
        vec!["Estado de conservação desconhecido".to_owned()]
    );
}

#[test]
fn the_answer_may_arrive_in_several_parts_and_thought_parts_are_ignored() {
    let text = answer(vec![item(10, 1), item(20, 2), item(30, 3)]).to_string();
    let middle = text
        .char_indices()
        .nth(text.chars().count() / 2)
        .map_or(0, |(at, _)| at);
    let (first, second) = text.split_at(middle);
    let body = json!({
        "candidates": [{
            "content": { "parts": [
                { "text": "raciocínio interno que não faz parte da resposta", "thought": true },
                { "text": first },
                { "text": second }
            ] },
            "finishReason": "STOP"
        }]
    })
    .to_string();
    assert_eq!(parse_answer(&body, &sent()).unwrap().notes.len(), 3);
}

#[test]
fn a_single_candidate_ranks_as_one() {
    let one = vec![candidate(7, "Casa.")];
    let body = body_of(&answer(vec![item(7, 1)]));
    assert_eq!(parse_answer(&body, &one).unwrap().notes[0].ai_rank, 1);
}

// ---- the envelope --------------------------------------------------------------------------

#[test]
fn an_answer_cut_off_by_the_output_limit_is_truncated_not_half_read() {
    let body = json!({
        "candidates": [{
            "content": { "parts": [{ "text": "{\"ranking\": [{\"id\": 10" }] },
            "finishReason": "MAX_TOKENS"
        }]
    })
    .to_string();
    assert_eq!(malformed(&body), AiFailure::Truncated);
}

#[test]
fn a_blocked_answer_names_the_reason() {
    for reason in [
        "SAFETY",
        "RECITATION",
        "BLOCKLIST",
        "PROHIBITED_CONTENT",
        "SPII",
    ] {
        let body = json!({
            "candidates": [{ "content": { "parts": [] }, "finishReason": reason }]
        })
        .to_string();
        match malformed(&body) {
            AiFailure::Blocked(why) => assert!(why.contains(reason), "{why}"),
            other => panic!("{reason}: {other:?}"),
        }
    }
}

#[test]
fn a_blocked_prompt_has_no_candidates_and_is_reported_as_blocked() {
    let body = json!({ "promptFeedback": { "blockReason": "OTHER" } }).to_string();
    match malformed(&body) {
        AiFailure::Blocked(why) => assert!(why.contains("OTHER"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn bodies_that_are_not_an_answer_are_malformed() {
    for body in [
        "",
        "not json",
        "[]",
        "{}",
        r#"{"candidates": []}"#,
        r#"{"candidates": [{}]}"#,
        r#"{"candidates": [{"content": {"parts": []}, "finishReason": "STOP"}]}"#,
        r#"{"candidates": [{"content": {"parts": [{"text": "  "}]}, "finishReason": "STOP"}]}"#,
        r#"{"candidates": [{"content": {"parts": [{"text": "{}"}]}}]}"#,
        r#"{"candidates": [{"content": {"parts": [{"text": "{}"}]}, "finishReason": "OTHER"}]}"#,
    ] {
        assert!(
            matches!(malformed(body), AiFailure::Malformed(_)),
            "{body:?}"
        );
    }
}

#[test]
fn text_that_is_not_the_requested_json_is_malformed() {
    for text in [
        "Aqui está o ranking: 1) casa 10",
        "```json\n{\"ranking\": []}\n```",
        "{\"ranking\": [",
        "null",
        "{\"ranking\": \"x\"}",
        "{}",
        r#"{"ranking": [], "extra": 1}"#,
        r#"{"ranking": [{"id": 10}]}"#,
        r#"{"ranking": [{"id": "10", "rank": 1, "explanation": "x", "positives": [], "concerns": [], "uncertainties": []}]}"#,
        r#"{"ranking": [{"id": 10, "rank": 1.5, "explanation": "x", "positives": [], "concerns": [], "uncertainties": []}]}"#,
        r#"{"ranking": [{"id": 10, "rank": 1, "explanation": "x", "positives": [], "concerns": [], "uncertainties": [], "note": "y"}]}"#,
        r#"{"ranking": [{"id": 10, "rank": 1, "explanation": "x", "positives": ["texto solto"], "concerns": [], "uncertainties": []}]}"#,
    ] {
        let failure = malformed(&envelope(text));
        assert!(
            matches!(failure, AiFailure::Malformed(_)),
            "{text:?} -> {failure:?}"
        );
    }
}

// ---- the set of properties -----------------------------------------------------------------

#[test]
fn an_unknown_duplicate_or_missing_property_rejects_the_whole_answer() {
    let why = rejected(&answer(vec![item(10, 1), item(20, 2), item(99, 3)]));
    assert!(why.contains("99") && why.contains("not among"), "{why}");
    let why = rejected(&answer(vec![item(10, 1), item(10, 2), item(20, 3)]));
    assert!(
        why.contains("10") && why.contains("more than once"),
        "{why}"
    );
    let why = rejected(&answer(vec![item(10, 1), item(20, 2)]));
    assert!(why.contains("30") && why.contains("missing"), "{why}");
    let why = rejected(&answer(vec![]));
    assert!(why.contains("missing"), "{why}");
    let why = rejected(&answer(vec![
        item(10, 1),
        item(20, 2),
        item(30, 3),
        item(10, 4),
    ]));
    assert!(why.contains("more than once"), "{why}");
}

#[test]
fn ranks_must_be_exactly_one_to_n() {
    for answer in [
        answer(vec![item(10, 1), item(20, 1), item(30, 2)]), // duplicate
        answer(vec![item(10, 1), item(20, 2), item(30, 4)]), // gap
    ] {
        assert!(
            rejected(&answer).contains("ranks"),
            "the rank rule must be what refuses it"
        );
    }
    rejected(&answer(vec![item(10, 1), item(20, 1), item(30, 2)])); // duplicate
    rejected(&answer(vec![item(10, 1), item(20, 2), item(30, 4)])); // gap
    rejected(&answer(vec![item(10, 0), item(20, 1), item(30, 2)])); // zero
    rejected(&answer(vec![item(10, 2), item(20, 3), item(30, 4)])); // starts at 2
    rejected(&answer(vec![item(10, -1), item(20, 1), item(30, 2)])); // negative
    rejected(&answer(vec![
        item(10, 1),
        item(20, 2),
        item(30, 2_000_000_000_000),
    ])); // huge
}

// ---- bounds --------------------------------------------------------------------------------

fn with(mut base: Value, key: &str, value: Value) -> Value {
    base["ranking"][0][key] = value;
    base
}

fn three() -> Value {
    answer(vec![item(10, 1), item(20, 2), item(30, 3)])
}

#[test]
fn an_explanation_must_be_present_and_within_its_bound() {
    rejected(&with(three(), "explanation", json!("")));
    rejected(&with(three(), "explanation", json!("   \n ")));
    rejected(&with(
        three(),
        "explanation",
        json!("a".repeat(MAX_EXPLANATION_CHARS + 1)),
    ));
    ok(&with(
        three(),
        "explanation",
        json!("a".repeat(MAX_EXPLANATION_CHARS)),
    ));
    // Characters, not bytes.
    ok(&with(
        three(),
        "explanation",
        json!("ã".repeat(MAX_EXPLANATION_CHARS)),
    ));
}

#[test]
fn the_lists_are_bounded_in_length_and_in_the_size_of_each_item_and_hold_no_blanks() {
    let many = |n: usize| json!(vec!["x"; n]);
    rejected(&with(three(), "concerns", many(MAX_ITEMS + 1)));
    rejected(&with(three(), "uncertainties", many(MAX_ITEMS + 1)));
    ok(&with(three(), "concerns", many(MAX_ITEMS)));
    rejected(&with(
        three(),
        "concerns",
        json!(["a".repeat(MAX_ITEM_CHARS + 1)]),
    ));
    ok(&with(
        three(),
        "concerns",
        json!(["a".repeat(MAX_ITEM_CHARS)]),
    ));
    rejected(&with(three(), "concerns", json!([""])));
    rejected(&with(three(), "uncertainties", json!(["  "])));
    let positives = |n: usize| {
        json!(
            (0..n)
                .map(|_| json!({ "claim": "tem piscina", "evidence": "piscina" }))
                .collect::<Vec<_>>()
        )
    };
    rejected(&with(three(), "positives", positives(MAX_ITEMS + 1)));
    rejected(&with(
        three(),
        "positives",
        json!([{ "claim": "a".repeat(MAX_ITEM_CHARS + 1), "evidence": "piscina" }]),
    ));
    rejected(&with(
        three(),
        "positives",
        json!([{ "claim": "", "evidence": "piscina" }]),
    ));
}

// ---- evidence ------------------------------------------------------------------------------

fn positive(claim: &str, evidence: &str) -> Value {
    json!({ "claim": claim, "evidence": evidence })
}

#[test]
fn a_positive_whose_quote_is_in_the_text_is_kept() {
    let v = ok(&with(
        three(),
        "positives",
        json!([positive("Tem piscina", "casa com piscina")]),
    ));
    assert_eq!(
        v.notes[0].positives,
        vec![Positive {
            claim: "Tem piscina".into(),
            evidence: "casa com piscina".into()
        }]
    );
    assert!(v.warnings.is_empty());
}

#[test]
fn the_quote_is_matched_ignoring_case_accents_and_spacing() {
    for quote in [
        "CASA COM PISCINA",
        "casa  com\npiscina",
        "Próxima ao PARQUE",
        "proxima ao parque",
        "quintal amplo,",
    ] {
        let v = ok(&with(three(), "positives", json!([positive("x", quote)])));
        assert_eq!(
            v.notes[0].positives.len(),
            1,
            "{quote:?} should have matched"
        );
    }
}

#[test]
fn a_positive_whose_quote_is_not_in_the_text_is_dropped_and_reported() {
    let v = ok(&with(
        three(),
        "positives",
        json!([
            positive("Tem piscina", "casa com piscina"),
            positive("Tem academia", "academia completa"),
            positive("Vista para o mar", "vista para o mar"),
        ]),
    ));
    let kept: Vec<&str> = v.notes[0]
        .positives
        .iter()
        .map(|p| p.claim.as_str())
        .collect();
    assert_eq!(kept, vec!["Tem piscina"]);
    assert_eq!(v.warnings.len(), 2, "{:?}", v.warnings);
    for (warning, claim) in v.warnings.iter().zip(["Tem academia", "Vista para o mar"]) {
        assert!(warning.contains("10"), "names the property: {warning}");
        assert!(warning.contains(claim), "names the claim: {warning}");
    }
    // The rest of the answer is untouched.
    assert_eq!(v.notes.len(), 3);
    assert_eq!(v.notes[0].concerns.len(), 1);
}

#[test]
fn a_quote_must_come_from_that_propertys_own_text_not_another_one() {
    // "churrasqueira" is in property 20's text, not property 10's.
    let v = ok(&with(
        three(),
        "positives",
        json!([positive("Tem churrasqueira", "churrasqueira")]),
    ));
    assert!(v.notes[0].positives.is_empty());
    assert_eq!(v.warnings.len(), 1);
}

#[test]
fn an_empty_or_one_letter_quote_proves_nothing() {
    for quote in ["", "   ", "a", "ca"] {
        let v = ok(&with(three(), "positives", json!([positive("x", quote)])));
        assert!(
            v.notes[0].positives.is_empty(),
            "{quote:?} must not count as evidence"
        );
        assert_eq!(v.warnings.len(), 1, "{quote:?}");
    }
    let v = ok(&with(three(), "positives", json!([positive("x", "casa")])));
    assert_eq!(v.notes[0].positives.len(), 1, "a real word does");
}

#[test]
fn a_property_with_no_text_cannot_have_a_supported_positive() {
    let none = vec![candidate(1, "")];
    let body = body_of(&answer(vec![json!({
        "id": 1, "rank": 1, "explanation": "x",
        "positives": [positive("Tem piscina", "piscina")],
        "concerns": [], "uncertainties": []
    })]));
    let v = parse_answer(&body, &none).unwrap();
    assert!(v.notes[0].positives.is_empty());
    assert_eq!(v.warnings.len(), 1);
}

#[test]
fn a_quote_that_is_only_a_fragment_of_a_word_is_not_evidence() {
    // "pisc" is inside "piscina", "ina e quintal" starts mid-word, "amplo, proxima ao parq" ends mid-word.
    for quote in ["pisc", "ina e quintal", "quintal amplo, proxima ao parq"] {
        let v = ok(&with(three(), "positives", json!([positive("x", quote)])));
        assert!(
            v.notes[0].positives.is_empty(),
            "{quote:?} is not a quotation"
        );
        assert_eq!(v.warnings.len(), 1, "{quote:?}");
    }
    // The same words at word edges are fine, including a quote that ends in punctuation.
    for quote in ["piscina", "e quintal", "quintal amplo, proxima ao parque."] {
        let v = ok(&with(three(), "positives", json!([positive("x", quote)])));
        assert_eq!(
            v.notes[0].positives.len(),
            1,
            "{quote:?} should have matched"
        );
    }
}

#[test]
fn a_repeated_fragment_only_counts_if_one_occurrence_is_at_word_edges() {
    let repeated = vec![candidate(5, "supiscina e depois uma piscina nova")];
    let body = body_of(&answer(vec![json!({
        "id": 5, "rank": 1, "explanation": "x",
        "positives": [positive("Tem piscina", "piscina")],
        "concerns": [], "uncertainties": []
    })]));
    assert_eq!(
        parse_answer(&body, &repeated).unwrap().notes[0]
            .positives
            .len(),
        1
    );
}

#[test]
fn a_whole_word_that_is_too_short_is_still_not_evidence() {
    // "e" and "ao" are whole words of property 10's text, but a word that short supports nothing.
    for quote in ["e", "ao"] {
        let v = ok(&with(three(), "positives", json!([positive("x", quote)])));
        assert!(
            v.notes[0].positives.is_empty(),
            "{quote:?} must not count as evidence"
        );
        assert_eq!(v.warnings.len(), 1, "{quote:?}");
    }
}

#[test]
fn a_quote_longer_than_the_bound_rejects_the_answer_even_when_it_is_in_the_text() {
    let long_text = "casa ".repeat(120); // 600 characters, so a 501-character quote exists in it
    let one = vec![candidate(1, long_text.trim())];
    let quote = long_text.trim()[..MAX_ITEM_CHARS + 1].trim().to_owned();
    assert!(quote.chars().count() > MAX_ITEM_CHARS - 5);
    let too_long = format!("{quote} casa casa");
    let body = body_of(&answer(vec![json!({
        "id": 1, "rank": 1, "explanation": "x",
        "positives": [positive("x", &too_long[..MAX_ITEM_CHARS + 1])],
        "concerns": [], "uncertainties": []
    })]));
    match parse_answer(&body, &one) {
        Err(AiFailure::OutputRejected(why)) => assert!(why.contains("evidence"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_valid_answer_without_a_finish_reason_is_not_trusted() {
    let body = json!({
        "candidates": [{
            "content": { "parts": [{ "text": answer(vec![item(10, 1), item(20, 2), item(30, 3)]).to_string() }] }
        }]
    })
    .to_string();
    match malformed(&body) {
        AiFailure::Malformed(why) => assert!(why.contains("finish reason"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_answer_with_only_blank_text_says_it_is_empty() {
    let body = envelope("   \n ");
    match malformed(&body) {
        AiFailure::Malformed(why) => assert!(why.contains("empty"), "{why}"),
        other => panic!("{other:?}"),
    }
}
