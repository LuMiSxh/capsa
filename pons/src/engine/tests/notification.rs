// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn replayed_task_notifications_are_dropped_and_the_rest_is_untouched() {
    // The shape of a session/load replay: whole user messages between
    // the main agent's chunks, and a load response at the end.
    let t0 = Instant::now();
    let mut e = engine();
    let mut shown = Vec::new();
    for raw in [
        user_chunk("Launch two agents"),
        chunk("Launching."),
        user_chunk(NOTE),
        chunk("A is done."),
        user_chunk(NOTE),
        chunk("B is done."),
    ] {
        shown.extend(e.on_agent(&raw, t0));
    }
    shown.extend(e.on_agent(&response(7), t0));
    assert_eq!(
        texts(&shown),
        [
            "Launch two agents",
            "Launching.",
            "A is done.",
            "B is done."
        ]
    );
    assert_eq!(client_lines(&shown).len(), 5, "and the response");
    assert!(e.notes.is_empty());
}

#[test]
fn user_text_is_forwarded_without_waiting() {
    let mut e = engine();
    let raw = user_chunk("what does <task-notification> mean?");
    assert_eq!(
        e.on_agent(&raw, Instant::now()),
        vec![Action::ToClient {
            line: raw,
            synthesized: false
        }]
    );
}

#[test]
fn a_split_notification_is_dropped_and_a_false_start_is_released_in_order() {
    let t0 = Instant::now();
    let mut e = engine();
    let (a, rest) = NOTE.split_at(8);
    let (b, c) = rest.split_at(40);
    assert!(e.on_agent(&user_chunk(a), t0).is_empty());
    assert!(e.on_agent(&user_chunk(b), t0).is_empty());
    let out = e.on_agent(&user_chunk(c), t0);
    assert!(client_lines(&out).is_empty());

    let first = user_chunk("<task");
    let second = user_chunk("s are fun");
    assert!(e.on_agent(&first, t0).is_empty());
    let out = e.on_agent(&second, t0);
    let lines: Vec<_> = out
        .iter()
        .filter_map(|a| match a {
            Action::ToClient { line, .. } => Some(line.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(lines, [first, second]);
}

#[test]
fn an_unfinished_notification_is_released_by_the_next_message_or_the_tick() {
    let t0 = Instant::now();
    let mut e = engine();
    let half = user_chunk(NOTE.split_at(50).0);
    assert!(e.on_agent(&half, t0).is_empty());
    let out = e.on_agent(&chunk("next"), t0 + ms(10));
    let lines = client_lines(&out);
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0]["params"]["update"]["sessionUpdate"],
        "user_message_chunk"
    );
    assert_eq!(texts(&out)[1], "next");

    assert!(e.on_agent(&half, t0).is_empty());
    assert!(client_lines(&e.tick(t0 + ms(900))).is_empty());
    assert_eq!(client_lines(&e.tick(t0 + secs(1))).len(), 1);
}

#[test]
fn keep_flag_forwards_task_notifications() {
    let mut e = Engine::new(Config {
        drop_task_notifications: false,
        ..test_config()
    });
    let raw = user_chunk(NOTE);
    assert_eq!(
        e.on_agent(&raw, Instant::now()),
        vec![Action::ToClient {
            line: raw,
            synthesized: false
        }]
    );
}

/// Replays the timing of round 3 of the logged Zed session (times in ms
/// from log second 975.000), ticking every 100 ms on the log's tick grid
/// (xx.069). Returns when the entries for A and B reached the client.
fn replay_round_3(cfg: Config) -> [u64; 2] {
    let events: &[(u64, Ev)] = &[
        (0, Ev::Prompt),
        (755, Ev::Launch("A")),
        (1539, Ev::Launch("B")),
        (2077, Ev::Child("A")),
        (2097, Ev::ChildEnd("A")),
        (6064, Ev::Child("B")),
        (6167, Ev::ChildEnd("B")),
        (9300, Ev::Prompt2),
        (6475, Ev::Chunk),
        (7000, Ev::Chunk),
        (8690, Ev::Chunk),
        (9362, Ev::Chunk),
        (9373, Ev::Marker),
        (9373, Ev::Response),
        (10412, Ev::Chunk),
        (11500, Ev::Chunk),
        (12047, Ev::Chunk),
        (12071, Ev::Marker),
    ];
    let t0 = Instant::now();
    let mut e = Engine::new(cfg);
    let mut at: [Option<u64>; 2] = [None, None];
    let mut seen = |out: &[Action], now: u64| {
        for m in entries(out) {
            let id = m["params"]["update"]["toolCallId"]
                .as_str()
                .unwrap()
                .to_owned();
            let slot = usize::from(id.ends_with("-B"));
            at[slot].get_or_insert(now);
        }
    };
    for now in 0..=15_000u64 {
        let t = t0 + ms(now);
        for (_, ev) in events.iter().filter(|(when, _)| *when == now) {
            let out = match ev {
                Ev::Prompt => e.on_client(&prompt(1), t),
                Ev::Launch(id) => {
                    let mut out = e.on_agent(&real_parent(id), t);
                    out.extend(e.on_agent(&real_launch_result(id), t));
                    out
                }
                Ev::Child(id) => e.on_agent(&kid(id, &format!("{id}1"), "Read"), t),
                Ev::ChildEnd(id) => e.on_agent(&kid_end(id, &format!("{id}1")), t),
                Ev::Prompt2 => e.on_client(&prompt(2), t),
                Ev::Chunk => e.on_agent(&chunk("text"), t),
                Ev::Marker => e.on_agent(&task_notification(), t),
                Ev::Response => e.on_agent(&response(1), t),
            };
            seen(&out, now);
        }
        if now % 100 == 69 {
            seen(&e.tick(t), now);
        }
    }
    [at[0].expect("A announced"), at[1].expect("B announced")]
}

#[test]
fn replay_of_the_logged_session_shows_the_lateness_and_its_fix() {
    // Before: 2 s settle wait after the marker, entry sent at once. This
    // reproduces the log: entries at 986.469 and 989.169.
    let before = replay_round_3(Config {
        notify_settle: secs(2),
        entry_gap: Duration::ZERO,
        ..test_config()
    });
    assert_eq!(before, [11_469, 14_169]);
    // After: the marker is the decision point.
    let after = replay_round_3(test_config());
    assert_eq!(after, [9_373, 12_071]);
    // Marker times: 9_373 and 12_071; last child updates 2_097 and 6_167.
    assert_eq!(
        [
            before[0] - 9_373,
            before[1] - 12_071,
            after[0] - 9_373,
            after[1] - 12_071
        ],
        [2_096, 2_098, 0, 0]
    );
}
