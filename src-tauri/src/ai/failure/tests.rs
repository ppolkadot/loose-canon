use super::*;
use crate::error::Error;

fn every_failure() -> Vec<AiFailure> {
    vec![
        AiFailure::Disabled,
        AiFailure::NoKey,
        AiFailure::NoCandidates,
        AiFailure::Busy,
        AiFailure::Cancelled,
        AiFailure::Quota { retry_after: None },
        AiFailure::Unauthorized { status: 403 },
        AiFailure::RequestRefused {
            status: 404,
            message: "unknown model".into(),
        },
        AiFailure::Timeout,
        AiFailure::Network("connection refused".into()),
        AiFailure::Server { status: 503 },
        AiFailure::Blocked("SAFETY".into()),
        AiFailure::Truncated,
        AiFailure::Malformed("not JSON".into()),
        AiFailure::OutputRejected("duplicate id".into()),
    ]
}

#[test]
fn every_failure_has_its_own_kind_and_a_readable_message() {
    let failures = every_failure();
    let mut kinds: Vec<&str> = failures.iter().map(AiFailure::kind).collect();
    assert!(kinds.iter().all(|k| k.starts_with("ai_")), "{kinds:?}");
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), failures.len(), "two failures share a kind");
    for failure in &failures {
        assert!(!failure.to_string().is_empty());
    }
}

#[test]
fn the_wait_a_quota_error_reports_is_shown_to_the_user() {
    let without = AiFailure::Quota { retry_after: None }.to_string();
    assert!(!without.contains("try again"), "{without}");
    let with = AiFailure::Quota {
        retry_after: Some(Duration::from_secs(42)),
    }
    .to_string();
    assert!(with.contains("42 s"), "{with}");
    let rounded_up = AiFailure::Quota {
        retry_after: Some(Duration::from_millis(100)),
    }
    .to_string();
    assert!(
        rounded_up.contains("1 s"),
        "never promises an instant retry: {rounded_up}"
    );
}

#[test]
fn a_failure_reaches_the_interface_under_its_own_kind() {
    for failure in every_failure() {
        let kind = failure.kind();
        let error = Error::Ai(failure);
        assert_eq!(error.kind(), kind);
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["kind"], kind);
        assert!(json["message"].as_str().is_some_and(|m| !m.is_empty()));
    }
}
