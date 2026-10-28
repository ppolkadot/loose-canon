use super::*;

fn with(model: &str, candidate_count: usize) -> AiSettings {
    AiSettings {
        enabled: true,
        model: model.to_owned(),
        candidate_count,
    }
}

#[test]
fn the_ai_is_off_by_default() {
    let defaults = AiSettings::default();
    assert!(!defaults.enabled, "sending data out must be opt-in");
    assert_eq!(defaults.model, DEFAULT_MODEL);
    assert!((MIN_CANDIDATES..=MAX_CANDIDATES).contains(&defaults.candidate_count));
    defaults.validate().unwrap();
}

#[test]
fn the_number_of_candidates_is_bounded_both_ways() {
    for ok in [MIN_CANDIDATES, 20, MAX_CANDIDATES] {
        with("gemini-x", ok).validate().unwrap();
    }
    for bad in [0, 1, MIN_CANDIDATES - 1, MAX_CANDIDATES + 1, 1000] {
        let error = with("gemini-x", bad).validate().unwrap_err();
        assert_eq!(error.kind(), "invalid_config", "{bad}");
    }
}

#[test]
fn model_names_are_plain_ids_because_they_become_part_of_a_url() {
    for ok in [
        "gemini-2.5-flash",
        "gemini_pro",
        "a",
        "Model-1.0",
        &"a".repeat(64),
    ] {
        with(ok, 20)
            .validate()
            .unwrap_or_else(|e| panic!("{ok:?}: {e}"));
    }
    for bad in [
        "",
        " ",
        "gemini 2",
        "models/gemini",
        "gemini?key=x",
        "gemini#x",
        "../v1/other",
        "a..b",
        "-flash",
        ".hidden",
        "gëmini",
        "gemini\n",
        "gemini:generateContent",
        &"a".repeat(65),
    ] {
        let error = with(bad, 20).validate().unwrap_err();
        assert_eq!(error.kind(), "invalid_config", "{bad:?}");
    }
}

#[test]
fn settings_are_read_and_written_with_camel_case_keys_and_unknown_fields_are_refused() {
    let json = serde_json::to_value(AiSettings::default()).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["candidateCount", "enabled", "model"]);
    let partial: AiSettings = serde_json::from_str(r#"{ "enabled": true }"#).unwrap();
    assert!(partial.enabled);
    assert_eq!(
        partial.model, DEFAULT_MODEL,
        "omitted fields keep their defaults"
    );
    assert!(serde_json::from_str::<AiSettings>(r#"{ "apiKey": "x" }"#).is_err());
    assert!(
        serde_json::from_str::<AiSettings>(r#"{ "baseUrl": "https://elsewhere.test" }"#).is_err(),
        "the endpoint is not a setting: the key must only ever go to the provider"
    );
}
