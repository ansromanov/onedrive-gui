use super::*;

#[test]
fn counts_progress() {
    let mut s = State::default();
    s.accounts.insert("a".into(), Account::default());
    s.apply("a", Event::Planned(2));
    s.apply(
        "a",
        Event::Progress {
            path: "big".into(),
            percent: 40,
        },
    );
    assert_eq!(
        (s.accounts["a"].current.as_str(), s.accounts["a"].percent),
        ("big", 40)
    );
    s.apply(
        "a",
        Event::Transfer {
            action: "download",
            path: "big".into(),
            ok: true,
        },
    );
    s.apply(
        "a",
        Event::Transfer {
            action: "delete-local",
            path: "x".into(),
            ok: true,
        },
    );
    let a = &s.accounts["a"];
    assert_eq!((a.planned, a.transferred, a.current.as_str()), (2, 1, ""));
    s.apply("a", Event::Synced);
    assert_eq!(
        (s.accounts["a"].planned, s.accounts["a"].transferred),
        (0, 0)
    );
}
