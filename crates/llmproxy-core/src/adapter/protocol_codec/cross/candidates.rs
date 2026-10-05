//! 按候选边界编码，不能把多个独立答案拼成一个答案。
use super::{Conversion, ResponseTarget, response::encode_single, unsupported, warn};
use crate::{
    adapter::Result,
    ir::response::{Candidate, Item, Response},
    protocol::{Protocol, Response as Raw},
};
use std::collections::HashSet;

/// 多候选目标逐个编码；单候选目标选择序号最小的候选，并明确报告信息损失。
pub(in crate::adapter::protocol_codec) fn encode_response(
    protocol: Protocol,
    response: &Response,
    target: &ResponseTarget<'_>,
) -> Result<Conversion<Raw>> {
    use crate::ir::response::Status;
    if matches!(response.status, Status::Failed | Status::Cancelled) {
        return super::response::encode_failure(protocol, response, target);
    }
    if response.failure.is_some() {
        return Err(unsupported(
            "response.failure",
            "正常完成状态不能包含生成错误",
        ));
    }
    if response.candidates.is_empty() {
        return Err(unsupported("candidates", "响应没有候选或可识别的结束原因"));
    }
    let mut seen_items = HashSet::new();
    let mut seen_indices = HashSet::new();
    for candidate in &response.candidates {
        if !seen_indices.insert(candidate.index) {
            return Err(unsupported("candidates.index", "候选序号重复"));
        }
        for index in &candidate.items {
            if *index >= response.items.len() || !seen_items.insert(*index) {
                return Err(unsupported("candidates.items", "候选输出项索引越界或重复"));
            }
        }
    }
    if seen_items.len() != response.items.len() {
        return Err(unsupported("candidates.items", "存在未归属候选的输出项"));
    }
    let message_indices = response
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Message(i) => Some(*i),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let message_count = response
        .items
        .iter()
        .filter(|item| matches!(item, Item::Message(_)))
        .count();
    if message_count != message_indices.len()
        || message_indices.len() != response.messages.len()
        || message_indices
            .iter()
            .any(|i| *i >= response.messages.len())
    {
        return Err(unsupported("items", "有消息未进入有序输出项或消息索引越界"));
    }
    let mut candidates = response.candidates.iter().collect::<Vec<_>>();
    candidates.sort_by_key(|c| c.index);
    let multiple = matches!(protocol, Protocol::OpenAiChat | Protocol::Gemini);
    let mut converted = encode_single(protocol, &select(response, candidates[0])?, target)?;
    set_index(&mut converted.body, candidates[0].index);
    if multiple {
        for candidate in &candidates[1..] {
            let mut next = encode_single(protocol, &select(response, candidate)?, target)?;
            set_index(&mut next.body, candidate.index);
            match (&mut converted.body, next.body) {
                (Raw::Chat(body), Raw::Chat(next)) => body.choices.extend(next.choices),
                (Raw::Gemini(body), Raw::Gemini(next)) => {
                    if let (
                        crate::protocol::OptionalNullable::Value(current),
                        crate::protocol::OptionalNullable::Value(next),
                    ) = (&mut body.candidates, next.candidates)
                    {
                        current.extend(next);
                    }
                }
                _ => unreachable!("目标协议固定"),
            }
            for warning in next.warnings {
                if !converted.warnings.contains(&warning) {
                    converted.warnings.push(warning);
                }
            }
        }
    } else if candidates.len() > 1 {
        warn(
            &mut converted.warnings,
            response.source_protocol(),
            protocol,
            "candidates",
            "目标协议只支持单候选，保留序号最小的候选；用量仍为上游所有候选的合计",
        );
    }
    Ok(converted)
}

/// 复制候选引用的消息并重建局部索引，保持每个候选内部的输出顺序。
fn select(response: &Response, candidate: &Candidate) -> Result<Response> {
    let mut selected = response.clone().without_source();
    selected.messages.clear();
    selected.items.clear();
    let mut seen = HashSet::new();
    for index in &candidate.items {
        selected.items.push(match &response.items[*index] {
            Item::Message(i) => {
                if !seen.insert(*i) {
                    return Err(unsupported("items", "消息索引重复"));
                }
                let message = response
                    .messages
                    .get(*i)
                    .ok_or_else(|| unsupported("items", "消息索引无效"))?;
                selected.messages.push(message.clone());
                Item::Message(selected.messages.len() - 1)
            }
            item => item.clone(),
        });
    }
    selected.candidates = vec![Candidate {
        index: 0,
        items: (0..selected.items.len()).collect(),
        finish_reason: candidate.finish_reason,
    }];
    Ok(selected)
}

/// 协议自身携带的候选序号不要求连续，不能用数组位置替代。
fn set_index(body: &mut Raw, index: u64) {
    match body {
        Raw::Chat(body) => body.choices[0].index = index,
        Raw::Gemini(body) => {
            if let crate::protocol::OptionalNullable::Value(candidates) = &mut body.candidates {
                candidates[0].index = crate::protocol::OptionalNullable::Value(index);
            }
        }
        _ => {}
    }
}
