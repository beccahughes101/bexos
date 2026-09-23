use bexos_migration::{Error, codec::Encoder};
use bexos_usersd::auth::{RuntimeUserAuthProvider, TeeUserAuthProvider};
use bexos_usersd::migration::Runtime;
use bexos_userspace::{Channel, live_migration::State};

#[test]
fn transplant_retains_late_reply_fence_and_public_tee_sessions() {
    for pending in [false, true] {
        let mut source = Runtime::empty();
        source.auth = RuntimeUserAuthProvider::Tee(TeeUserAuthProvider::from_parts(
            Channel(321),
            Some(11),
            Some(12),
            pending,
        ));
        let bytes = source.encode_record(5).unwrap().unwrap();
        let mut candidate = Runtime::empty();
        candidate.adopt_record(5, Some(&bytes)).unwrap();
        let RuntimeUserAuthProvider::Tee(provider) = &candidate.auth else {
            panic!("provider lost during adoption");
        };
        assert_eq!(provider.client().0, 321);
        assert_eq!(provider.gatekeeper_session(), Some(11));
        assert_eq!(provider.keymint_session(), Some(12));
        assert_eq!(provider.awaiting_response(), pending);
        assert_eq!(candidate.encode_record(5).unwrap(), Some(bytes));
    }
}

fn record(words: &[u64]) -> Vec<u8> {
    let mut writer = Encoder::new();
    for word in words {
        writer.word(*word);
    }
    writer.finish()
}

#[test]
fn incompatible_or_malformed_provider_does_not_clear_existing_fence() {
    let mut candidate = Runtime::empty();
    candidate.auth = RuntimeUserAuthProvider::Tee(TeeUserAuthProvider::from_parts(
        Channel(321),
        Some(11),
        Some(12),
        true,
    ));
    let original = candidate.encode_record(5).unwrap();
    // The old record never tracked a timed-out request. It cannot be safely
    // treated as an empty queue when importing a live channel.
    assert_eq!(
        candidate.adopt_record(5, Some(&record(&[1, 321, 11, 12]))),
        Err(Error::UnsupportedVersion)
    );
    for words in [
        vec![2, 1, 321, 11, 12, 2],    // noncanonical bool
        vec![2, 1, 0, 11, 12, 1],      // no channel
        vec![2, 0, 0, 0, 0, 1],        // pending unsupported provider
        vec![2, 3, 321, 11, 12, 1],    // unknown provider
        vec![2, 1, 321, 11, 12, 1, 0], // trailing data
        vec![2, 1, 321, 11, 12],       // missing queue state
    ] {
        assert!(candidate.adopt_record(5, Some(&record(&words))).is_err());
        assert_eq!(candidate.encode_record(5).unwrap(), original);
    }
}
