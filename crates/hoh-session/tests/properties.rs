mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
fn build(seed: u64, noise: bool) -> Session {
    let s = session();
    let mut rng = seed.wrapping_add(1);
    for i in 0..25 {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let n = (rng >> 32) % 8;
        if noise || n < 3 {
            match n {
                0 => {
                    user(&s, &format!("user-{seed}-{i}-😀"));
                }
                1 | 2 => {
                    let role = if n == 1 { "assistant" } else { "tool" };
                    let t = if n == 1 {
                        "assistant/message"
                    } else {
                        "tool/result"
                    };
                    s.append(
                        t,
                        data(role, if rng & 1 == 0 { "" } else { "text" }),
                        Some(SurfaceIntent::append()),
                    )
                    .unwrap();
                }
                3 => {
                    s.append(
                        "assistant/attempt",
                        json!({"turn":1,"step":1,"stream":[]}),
                        None,
                    )
                    .unwrap();
                }
                _ => {
                    let t = ["turn/start", "turn/end", "step/start", "step/end"][(n - 4) as usize];
                    s.append(
                        t,
                        json!({"turn":1,"step":1,"reason":{"kind":"completed"}}),
                        None,
                    )
                    .unwrap();
                }
            }
        }
    }
    s
}
#[test]
fn derive_messages_is_deterministic() {
    for seed in 0..256 {
        let s = build(seed, true);
        assert_eq!(s.derive_messages().unwrap(), s.derive_messages().unwrap());
    }
}
#[test]
fn seq_is_strictly_monotonic_and_zero_based_contiguous() {
    for seed in 0..256 {
        let s = build(seed, true);
        let events = s.snapshot_events(None, None);
        assert_eq!(u64::from(s.seq()), events.len() as u64);
        for (i, e) in events.iter().enumerate() {
            assert_eq!(u64::from(e.seq()), i as u64);
        }
    }
}
#[test]
fn replay_from_seed_reproduces_the_derivation_identically() {
    for seed in 0..256 {
        let s = build(seed, true);
        let r = Session::create(
            "replay".into(),
            Some(&s.snapshot_events(None, None)),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(r.derive_messages().unwrap(), s.derive_messages().unwrap());
        assert_eq!(u64::from(r.seq()), u64::from(s.seq()) + 1);
        assert_eq!(
            r.snapshot_events(None, Some(s.seq())),
            s.snapshot_events(None, None)
        );
    }
}
#[test]
fn replaying_a_log_that_already_ends_in_end_seed_adds_no_further_marker() {
    for seed in 0..256 {
        let s = build(seed, true);
        let a = Session::create(
            "a".into(),
            Some(&s.snapshot_events(None, None)),
            None,
            None,
            None,
        )
        .unwrap();
        let b = Session::create(
            "b".into(),
            Some(&a.snapshot_events(None, None)),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(a.snapshot_events(None, None), b.snapshot_events(None, None));
    }
}
#[test]
fn non_message_events_never_affect_derived_history_in_any_interleaving() {
    for seed in 0..256 {
        assert_eq!(
            build(seed, true).derive_messages().unwrap(),
            build(seed, false).derive_messages().unwrap()
        );
    }
}
#[test]
fn every_derived_message_has_a_known_role_and_is_frozen() {
    for seed in 0..256 {
        let s = build(seed, true);
        let before = s.snapshot_events(None, None);
        for m in s.derive_messages().unwrap() {
            assert!(["user", "assistant", "tool", "system"].contains(&m["role"].as_str().unwrap()));
            let mut owned = (*m).clone();
            owned["content"] = json!([{"type":"text","text":"mutation"}]);
        }
        assert_eq!(before, s.snapshot_events(None, None));
    }
}
