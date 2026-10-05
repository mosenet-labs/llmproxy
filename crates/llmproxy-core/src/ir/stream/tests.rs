use super::{Event, Head, Key, Limits, Metadata, State, ToolHead, UsageState};
use crate::ir::{
    response::{Failure, FinishReason, Status},
    usage::Usage,
};

fn key(candidate: u64, part: u64) -> Key {
    Key {
        candidate,
        item: 0,
        part,
    }
}

fn started(limits: Limits) -> State {
    let mut state = State::new(limits);
    state.accept(&Event::Start(Metadata::default())).unwrap();
    state.accept(&Event::CandidateStart(0)).unwrap();
    state
}

fn tool(state: &mut State, key: Key, name: Option<&str>) {
    state
        .accept(&Event::PartStart {
            key,
            head: Head::Tool(ToolHead {
                name: name.map(str::to_owned),
                ..Default::default()
            }),
        })
        .unwrap();
}

fn arguments(key: Key, text: &str) -> Event {
    Event::ToolDelta {
        key,
        id: None,
        name: None,
        arguments: Some(text.into()),
    }
}

#[test]
fn interleaved_candidates_end_independently_before_final_usage() {
    let mut state = started(Limits::default());
    state.accept(&Event::CandidateStart(1)).unwrap();
    for index in 0..2 {
        state
            .accept(&Event::PartStart {
                key: key(index, 0),
                head: Head::Text,
            })
            .unwrap();
    }
    state
        .accept(&Event::TextDelta {
            key: key(1, 0),
            text: "第二候选".into(),
        })
        .unwrap();
    assert!(
        state
            .accept(&Event::CandidateEnd {
                index: 1,
                reason: FinishReason::Stop
            })
            .is_err()
    );
    state.accept(&Event::PartEnd(key(1, 0))).unwrap();
    state
        .accept(&Event::CandidateEnd {
            index: 1,
            reason: FinishReason::Stop,
        })
        .unwrap();
    assert!(state.accept(&Event::End(Status::Completed)).is_err());
    state
        .accept(&Event::TextDelta {
            key: key(0, 0),
            text: "第一候选".repeat(1024),
        })
        .unwrap();
    assert_eq!(state.buffered_bytes(), 0);
    state.accept(&Event::PartEnd(key(0, 0))).unwrap();
    state
        .accept(&Event::CandidateEnd {
            index: 0,
            reason: FinishReason::Length,
        })
        .unwrap();
    state
        .accept(&Event::Usage(Box::new(Usage {
            output_tokens: Some(7),
            ..Default::default()
        })))
        .unwrap();
    state.accept(&Event::End(Status::Completed)).unwrap();
    state.accept(&Event::End(Status::Completed)).unwrap();
    assert_eq!(state.candidate(0), Some(Some(FinishReason::Length)));
    assert_eq!(state.usage().snapshot().unwrap().output_tokens, Some(7));
    assert!(state.accept(&Event::Heartbeat).is_err());
}

#[test]
fn parallel_tool_fragments_preserve_partial_json_and_late_headers() {
    let mut state = started(Limits::default());
    tool(&mut state, key(0, 0), None);
    tool(&mut state, key(0, 1), Some("other"));
    state.accept(&arguments(key(0, 0), "{\"x\":")).unwrap();
    state.accept(&arguments(key(0, 1), "{}")).unwrap();
    state
        .accept(&Event::ToolDelta {
            key: key(0, 0),
            id: Some("call-1".into()),
            name: Some("get_".into()),
            arguments: None,
        })
        .unwrap();
    state
        .accept(&Event::ToolDelta {
            key: key(0, 0),
            id: Some("call-1".into()),
            name: Some("value".into()),
            arguments: Some("1}".into()),
        })
        .unwrap();
    let part = state.part(key(0, 0)).unwrap();
    assert_eq!(part.arguments, "{\"x\":1}");
    assert!(matches!(&part.head, Head::Tool(head) if head.name.as_deref() == Some("get_value")));
    assert_eq!(state.part(key(0, 1)).unwrap().arguments, "{}");
    assert!(
        state
            .accept(&Event::ToolDelta {
                key: key(0, 0),
                id: Some("different".into()),
                name: None,
                arguments: None
            })
            .is_err()
    );
    state.accept(&Event::PartEnd(key(0, 1))).unwrap();
    state.accept(&Event::PartEnd(key(0, 0))).unwrap();
    assert_eq!(state.buffered_bytes(), 0);
    assert_eq!(state.part(key(0, 0)).unwrap().arguments.capacity(), 0);
}

#[test]
fn rejected_buffer_growth_is_atomic_and_finished_blocks_release_budget() {
    let mut state = started(Limits {
        buffered_bytes: 5,
        entries: 4,
    });
    tool(&mut state, key(0, 0), Some("f"));
    state.accept(&arguments(key(0, 0), "{\"x")).unwrap();
    assert!(state.accept(&arguments(key(0, 0), "\":1}")).is_err());
    assert_eq!(state.part(key(0, 0)).unwrap().arguments, "{\"x");
    assert_eq!(state.buffered_bytes(), 4);
    state.accept(&Event::PartEnd(key(0, 0))).unwrap();
    tool(&mut state, key(0, 1), Some("f"));
    state.accept(&arguments(key(0, 1), "{}")).unwrap();
    state.accept(&Event::PartEnd(key(0, 1))).unwrap();
    tool(&mut state, key(0, 2), Some("f"));
    state.accept(&Event::PartEnd(key(0, 2))).unwrap();
    assert!(
        state
            .accept(&Event::PartStart {
                key: key(0, 3),
                head: Head::Text
            })
            .is_err()
    );
}

#[test]
fn failure_can_abort_open_tools_but_cannot_be_reported_as_success() {
    let mut state = started(Limits::default());
    tool(&mut state, key(0, 0), None);
    state.accept(&arguments(key(0, 0), "{")).unwrap();
    assert!(state.accept(&Event::PartEnd(key(0, 0))).is_err());
    assert!(state.accept(&Event::End(Status::Incomplete)).is_err());
    state
        .accept(&Event::Failure(Failure {
            code: "disconnect".into(),
            message: "private detail".into(),
        }))
        .unwrap();
    assert!(state.accept(&Event::End(Status::Completed)).is_err());
    state.accept(&Event::End(Status::Failed)).unwrap();
    assert_eq!(state.buffered_bytes(), 0);
    assert_eq!(state.ended(), Some(Status::Failed));
}

#[test]
fn cumulative_usage_keeps_missing_zero_cache_and_modality_distinct() {
    let mut state = UsageState::default();
    assert!(state.snapshot().is_none());
    let mut initial = Usage {
        input_tokens: Some(10),
        output_tokens: Some(2),
        total_tokens: Some(12),
        ..Default::default()
    };
    initial.cache.read_input_tokens = Some(0);
    initial.cache.write_input_tokens = Some(8);
    initial.cache.write_long_input_tokens = Some(8);
    initial.cache.read_details.image_tokens = Some(0);
    initial.output_details.reasoning_tokens = Some(1);
    state.update(&initial);
    state.update(&initial);
    assert_eq!(state.snapshot(), Some(&initial));
    state.update(&Usage {
        output_tokens: Some(5),
        ..Default::default()
    });
    let current = state.snapshot().unwrap();
    assert_eq!(current.input_tokens, Some(10));
    assert_eq!(current.output_tokens, Some(5));
    assert_eq!(current.total_tokens, None);
    assert_eq!(current.cache.write_long_input_tokens, Some(8));
    assert_eq!(current.cache.write_short_input_tokens, None);
    assert_eq!(current.cache.read_input_tokens, Some(0));
    assert_eq!(current.cache.read_details.image_tokens, Some(0));
    assert_eq!(current.output_details.reasoning_tokens, Some(1));
    state.update(&Usage {
        input_tokens: Some(0),
        output_tokens: Some(0),
        total_tokens: Some(0),
        ..Default::default()
    });
    assert_eq!(state.snapshot().unwrap().total_tokens, Some(0));
}
