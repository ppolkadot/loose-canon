use super::*;

const KEY: &str = "AIzaSyTESTKEY-not-a-real-key_0123456789";

fn parse(raw: &str) -> ApiKey {
    ApiKey::parse(raw).expect("a valid key")
}

#[test]
fn a_pasted_key_is_trimmed_and_kept_as_is_otherwise() {
    assert_eq!(parse(&format!("  {KEY}\r\n")).expose(), KEY);
}

#[test]
fn keys_that_could_not_be_header_values_or_are_empty_are_refused() {
    for bad in [
        "",
        "   ",
        "\n",
        "two words",
        "tab\tinside",
        "newline\ninside",
        "accént",
        "null\0byte",
        &"k".repeat(257),
    ] {
        let error = ApiKey::parse(bad).unwrap_err();
        assert_eq!(error.kind(), "invalid_config", "{bad:?}");
        assert!(
            !error.to_string().contains(bad.trim()) || bad.trim().is_empty(),
            "the refusal repeats the key: {error}"
        );
    }
    ApiKey::parse(&"k".repeat(256)).unwrap();
}

#[test]
fn a_key_never_shows_in_debug_output() {
    let key = parse(KEY);
    let shown = format!("{key:?} {:?}", Some(&key));
    assert!(!shown.contains("AIza") && !shown.contains(KEY), "{shown}");
    assert!(shown.contains("redacted"));
}

#[test]
fn the_memory_store_saves_replaces_and_clears() {
    let store = MemorySecretStore::default();
    assert!(store.get().unwrap().is_none());
    assert!(!store.is_configured().unwrap());

    store.set(&parse(KEY)).unwrap();
    assert!(store.is_configured().unwrap());
    assert_eq!(store.get().unwrap().unwrap().expose(), KEY);

    store.set(&parse("another-key")).unwrap();
    assert_eq!(store.get().unwrap().unwrap().expose(), "another-key");

    store.clear().unwrap();
    assert!(store.get().unwrap().is_none());
    store.clear().unwrap();
}

/// Talks to the real Windows Credential Manager under a throwaway name and always removes the
/// entry afterwards, so it neither touches the application's own credential nor leaves one behind.
#[cfg(windows)]
mod windows_credential_manager {
    use super::*;

    struct Cleanup(String);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Ok(store) = WindowsCredentialStore::new(&self.0) {
                let _ = store.clear();
            }
        }
    }

    fn unique_service(test: &str) -> String {
        format!("loose-canon-test-{test}-{}", std::process::id())
    }

    #[test]
    fn a_key_survives_a_new_handle_which_is_what_a_restart_looks_like() {
        let service = unique_service("persist");
        let _cleanup = Cleanup(service.clone());

        let first = WindowsCredentialStore::new(&service).unwrap();
        assert!(first.get().unwrap().is_none(), "a fresh name starts empty");
        first.set(&parse(KEY)).unwrap();
        drop(first);

        let second = WindowsCredentialStore::new(&service).unwrap();
        assert_eq!(
            second.get().unwrap().expect("the key was kept").expose(),
            KEY
        );
        assert!(second.is_configured().unwrap());
    }

    #[test]
    fn the_credential_stays_on_this_pc_instead_of_roaming_with_a_domain_profile() {
        let service = unique_service("local");
        let _cleanup = Cleanup(service.clone());
        let store = WindowsCredentialStore::new(&service).unwrap();
        store.set(&parse(KEY)).unwrap();
        let attributes = store.attributes().unwrap();
        assert_eq!(
            attributes.get("persistence").map(String::as_str),
            Some("Local"),
            "{attributes:?}"
        );
        assert!(
            attributes.values().all(|v| !v.contains(KEY)),
            "the secret must not be readable as an attribute"
        );
    }

    #[test]
    fn setting_again_replaces_and_clearing_removes_and_is_repeatable() {
        let service = unique_service("replace");
        let _cleanup = Cleanup(service.clone());
        let store = WindowsCredentialStore::new(&service).unwrap();

        store.set(&parse("first-key")).unwrap();
        store.set(&parse("second-key")).unwrap();
        assert_eq!(store.get().unwrap().unwrap().expose(), "second-key");

        store.clear().unwrap();
        assert!(store.get().unwrap().is_none());
        store.clear().unwrap();
    }

    #[test]
    fn different_service_names_do_not_share_a_key() {
        let a = unique_service("iso-a");
        let b = unique_service("iso-b");
        let (_ca, _cb) = (Cleanup(a.clone()), Cleanup(b.clone()));
        let store_a = WindowsCredentialStore::new(&a).unwrap();
        let store_b = WindowsCredentialStore::new(&b).unwrap();
        store_a.set(&parse(KEY)).unwrap();
        assert!(store_b.get().unwrap().is_none());
    }
}
