use pactern::host::*;
use std::{collections::BTreeSet, sync::Arc};
fn grant(tenant: &str) -> Grant {
    Grant {
        tenant: tenant.into(),
        operations: BTreeSet::from([Operation::Read, Operation::Write]),
    }
}
fn store() -> Arc<QuotaStore> {
    Arc::new(QuotaStore::new([("a".into(), 100), ("b".into(), 9)]).unwrap())
}
fn invocation(s: &Arc<QuotaStore>) -> Invocation {
    Invocation::record(
        s.clone(),
        vec![grant("a")],
        Budget::default(),
        "revision",
        "policy",
    )
}
#[test]
fn transactions_are_isolated_nonnegative_and_rollback_on_drop() {
    let s = store();
    {
        let mut i = invocation(&s);
        assert!(matches!(i.begin("b"), Err(HostError::Denied)));
        let h = i.begin("a").unwrap();
        assert_eq!(i.reserve(&h, -1), Err(HostError::InvalidAmount));
        assert!(!i.reserve(&h, 101).unwrap());
        assert!(i.reserve(&h, 30).unwrap());
        assert_eq!(i.read(&h).unwrap(), 70);
        assert_eq!(s.balance("a").unwrap(), 100);
    }
    assert_eq!(s.balance("a").unwrap(), 100);
    assert_eq!(s.balance("b").unwrap(), 9);
    assert!(QuotaStore::new([("a".into(), -1)]).is_err());
}
#[test]
fn concurrent_commits_never_over_reserve() {
    let s = store();
    let mut workers = vec![];
    for _ in 0..16 {
        let s = s.clone();
        workers.push(std::thread::spawn(move || {
            let mut count = 0;
            loop {
                let mut i = invocation(&s);
                let h = i.begin("a").unwrap();
                if !i.reserve(&h, 1).unwrap() {
                    break;
                }
                match i.commit(h) {
                    Ok(()) => count += 1,
                    Err(HostError::Conflict) => {}
                    other => panic!("{other:?}"),
                }
            }
            count
        }));
    }
    assert_eq!(
        workers.into_iter().map(|w| w.join().unwrap()).sum::<i32>(),
        100
    );
    assert_eq!(s.balance("a").unwrap(), 0);
    assert_eq!(s.balance("b").unwrap(), 9);
    assert_eq!(s.live_writes(), 100);
}
#[test]
fn nested_budgets_cancel_and_timeout_revoke_authority() {
    let s = store();
    let mut i = Invocation::record(
        s.clone(),
        vec![grant("a")],
        Budget {
            operations: 3,
            ..Budget::default()
        },
        "r",
        "p",
    );
    let h = i.begin("a").unwrap();
    assert_eq!(
        i.scoped(Budget::default(), |i| {
            i.read(&h)?;
            i.read(&h)
        }),
        Err(HostError::BudgetExceeded)
    );
    assert_eq!(i.active_handles(), 0);
    assert_eq!(i.read(&h), Err(HostError::Revoked));
    let mut i = invocation(&s);
    let h = i.begin("a").unwrap();
    i.reserve(&h, 1).unwrap();
    i.cancellation().cancel();
    assert_eq!(i.commit(h), Err(HostError::Revoked));
    assert_eq!(s.balance("a").unwrap(), 100);
    let mut i = Invocation::record(
        s.clone(),
        vec![grant("a")],
        Budget {
            timeout_ms: 0,
            ..Budget::default()
        },
        "r",
        "p",
    );
    assert!(matches!(i.begin("a"), Err(HostError::Timeout)));
    for budget in [
        Budget {
            bytes: 1,
            ..Budget::default()
        },
        Budget {
            memory_bytes: 1,
            ..Budget::default()
        },
    ] {
        let mut i = Invocation::record(s.clone(), vec![grant("a")], budget, "r", "p");
        assert!(matches!(i.begin("a"), Err(HostError::BudgetExceeded)));
    }
}
fn workflow(i: &mut Invocation) {
    let h = i.begin("a").unwrap();
    assert_eq!(i.read(&h).unwrap(), 100);
    assert!(i.reserve(&h, 7).unwrap());
    i.commit(h).unwrap();
}
#[test]
fn replay_substitutes_results_and_performs_zero_live_writes() {
    let s = store();
    let mut i = invocation(&s);
    workflow(&mut i);
    let trace = i.finish().unwrap();
    assert_eq!(s.live_writes(), 1);
    let mut replay = Invocation::replay(
        s.clone(),
        vec![grant("a")],
        Budget::default(),
        "revision",
        "policy",
        trace.clone(),
    )
    .unwrap();
    workflow(&mut replay);
    replay.finish().unwrap();
    assert_eq!(s.live_writes(), 1);
    assert_eq!(s.balance("a").unwrap(), 93);
    assert!(matches!(
        Invocation::replay(
            s.clone(),
            vec![],
            Budget::default(),
            "different",
            "policy",
            trace.clone()
        ),
        Err(HostError::ReplayMismatch)
    ));
    let mut bad = trace.clone();
    bad.entries[0].response = None;
    let mut replay = Invocation::replay(
        s.clone(),
        vec![grant("a")],
        Budget::default(),
        "revision",
        "policy",
        bad,
    )
    .unwrap();
    assert!(matches!(
        replay.begin("a"),
        Err(HostError::MissingRecording)
    ));
    let mut replay = Invocation::replay(
        s.clone(),
        vec![grant("a")],
        Budget::default(),
        "revision",
        "policy",
        trace,
    )
    .unwrap();
    let h = replay.begin("a").unwrap();
    assert_eq!(replay.reserve(&h, 8), Err(HostError::ReplayMismatch));
    assert_eq!(s.live_writes(), 1);
}
#[test]
fn handles_cannot_cross_invocations_and_read_grants_cannot_write() {
    let s = store();
    let mut a = invocation(&s);
    let h = a.begin("a").unwrap();
    let mut b = invocation(&s);
    assert_eq!(b.read(&h), Err(HostError::Revoked));
    let mut i = Invocation::record(
        s.clone(),
        vec![Grant {
            tenant: "a".into(),
            operations: BTreeSet::from([Operation::Read]),
        }],
        Budget::default(),
        "r",
        "p",
    );
    let h = i.begin("a").unwrap();
    assert_eq!(i.reserve(&h, 1), Err(HostError::Denied));
}

#[test]
fn child_handles_expire_and_unwinding_revokes_all_authority() {
    let s = store();
    let mut i = invocation(&s);
    let parent = i.begin("a").unwrap();
    let child = i.scoped(Budget::default(), |i| i.begin("a")).unwrap();
    assert_eq!(i.read(&child), Err(HostError::Revoked));
    assert_eq!(i.read(&parent).unwrap(), 100);
    assert_eq!(i.active_handles(), 1);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), HostError> = i.scoped(Budget::default(), |i| {
            let h = i.begin("a")?;
            i.reserve(&h, 50)?;
            panic!("simulated trapped host work")
        });
    }));
    assert!(panic.is_err());
    assert_eq!(i.active_handles(), 0);
    assert_eq!(i.read(&parent), Err(HostError::Revoked));
    assert_eq!(s.balance("a").unwrap(), 100);
    assert_eq!(s.live_writes(), 0);
}

#[test]
fn missing_redacted_and_malformed_recordings_revoke_existing_handles() {
    let s = store();
    let mut live = invocation(&s);
    let h = live.begin("a").unwrap();
    live.read(&h).unwrap();
    let trace = live.finish().unwrap();
    for variant in 0..4 {
        let mut broken = trace.clone();
        match variant {
            0 => {
                broken.entries.pop();
            }
            1 => broken.entries[1].response = None,
            2 => broken.entries[1].response = Some(Ok(Response::Balance("-1".into()))),
            _ => broken.entries[1].response = Some(Ok(Response::Balance("0100".into()))),
        }
        let mut replay = Invocation::replay(
            s.clone(),
            vec![grant("a")],
            Budget::default(),
            "revision",
            "policy",
            broken,
        )
        .unwrap();
        let h = replay.begin("a").unwrap();
        assert!(matches!(
            replay.read(&h),
            Err(HostError::MissingRecording | HostError::ReplayMismatch)
        ));
        assert_eq!(replay.active_handles(), 0);
        assert_eq!(replay.read(&h), Err(HostError::Revoked));
    }
    assert_eq!(s.live_writes(), 0);
}

#[test]
fn failed_commit_replay_consumes_token_without_external_writes() {
    let s = store();
    let mut first = invocation(&s);
    let mut second = invocation(&s);
    let a = first.begin("a").unwrap();
    let b = second.begin("a").unwrap();
    first.reserve(&a, 1).unwrap();
    second.reserve(&b, 1).unwrap();
    first.commit(a).unwrap();
    assert_eq!(second.commit(b), Err(HostError::Conflict));
    assert_eq!(second.active_handles(), 0);
    let trace = second.finish().unwrap();
    let writes = s.live_writes();
    let mut replay = Invocation::replay(
        s.clone(),
        vec![grant("a")],
        Budget::default(),
        "revision",
        "policy",
        trace,
    )
    .unwrap();
    let b = replay.begin("a").unwrap();
    replay.reserve(&b, 1).unwrap();
    assert_eq!(replay.commit(b), Err(HostError::Conflict));
    assert_eq!(replay.active_handles(), 0);
    replay.finish().unwrap();
    assert_eq!(s.live_writes(), writes);
}

#[test]
fn scope_completion_observes_cancel_and_zero_time_budget() {
    let s = store();
    let mut i = invocation(&s);
    assert_eq!(
        i.scoped(
            Budget {
                timeout_ms: 0,
                ..Budget::default()
            },
            |_| Ok(())
        ),
        Err(HostError::Timeout)
    );
    let mut i = invocation(&s);
    assert_eq!(
        i.scoped(Budget::default(), |i| {
            i.cancellation().cancel();
            Ok(())
        }),
        Err(HostError::Revoked)
    );
}

#[test]
fn guest_checkpoints_revoke_without_consuming_host_operation_budget() {
    let s = store();
    let mut i = Invocation::record(
        s.clone(),
        vec![grant("a")],
        Budget {
            operations: 2,
            ..Budget::default()
        },
        "r",
        "p",
    );
    let h = i.begin("a").unwrap();
    for _ in 0..100 {
        i.checkpoint().unwrap();
    }
    assert_eq!(i.read(&h).unwrap(), 100);
    assert_eq!(i.read(&h), Err(HostError::BudgetExceeded));
    let mut i = invocation(&s);
    let h = i.begin("a").unwrap();
    i.cancellation().cancel();
    assert_eq!(i.checkpoint(), Err(HostError::Revoked));
    assert_eq!(i.active_handles(), 0);
    assert_eq!(i.read(&h), Err(HostError::Revoked));
    let mut i = Invocation::record(
        s,
        vec![grant("a")],
        Budget {
            timeout_ms: 0,
            ..Budget::default()
        },
        "r",
        "p",
    );
    assert_eq!(i.checkpoint(), Err(HostError::Timeout));
}

#[test]
fn lexical_release_is_unmetered_replayable_and_safe_after_revocation() {
    let s = store();
    let mut live = Invocation::record(
        s.clone(),
        vec![grant("a")],
        Budget {
            operations: 1,
            ..Budget::default()
        },
        "revision",
        "policy",
    );
    let handle = live.begin("a").unwrap();
    live.release(handle).unwrap();
    assert_eq!(live.active_handles(), 0);
    let trace = live.finish().unwrap();
    assert_eq!(trace.entries.len(), 1);
    let mut replay = Invocation::replay(
        s.clone(),
        vec![grant("a")],
        Budget {
            operations: 1,
            ..Budget::default()
        },
        "revision",
        "policy",
        trace,
    )
    .unwrap();
    let handle = replay.begin("a").unwrap();
    replay.release(handle).unwrap();
    replay.finish().unwrap();
    let mut revoked = invocation(&s);
    let handle = revoked.begin("a").unwrap();
    revoked.revoke();
    revoked.release(handle).unwrap();
    let mut foreign = invocation(&s);
    let handle = foreign.begin("a").unwrap();
    assert_eq!(revoked.release(handle), Err(HostError::Revoked));
    assert_eq!(s.live_writes(), 0);
}
