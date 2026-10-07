//! 按实际 Provider 与模型汇总已报告值，缺失值不补零。
use super::{Turn, UsageState};
use std::collections::BTreeMap;

/// 合计与报告覆盖率分别记录。
#[derive(Clone, Debug, Default)]
pub struct Counter {
    pub total: Option<u128>,
    pub reported: u64,
}
impl Counter {
    fn add(&mut self, value: Option<u64>) {
        if let Some(value) = value {
            self.total = Some(self.total.unwrap_or(0) + u128::from(value));
            self.reported += 1;
        }
    }
}
/// 单个会话或模型分组的用量查询投影。
#[derive(Clone, Debug, Default)]
pub struct Totals {
    pub turns: u64,
    pub partial: u64,
    pub unreported: u64,
    pub input: Counter,
    pub output: Counter,
    pub total: Counter,
    pub read: Counter,
    pub write: Counter,
    pub reasoning: Counter,
    hit_input: u128,
    hit_read: u128,
    pub hit_reported: u64,
}
impl Totals {
    /// 每轮只消费一份最终保存快照，累计输入包含重复发送的历史。
    pub fn add(&mut self, turn: &Turn) {
        self.turns += 1;
        match turn.usage_state {
            UsageState::Partial => self.partial += 1,
            UsageState::Unreported => self.unreported += 1,
            UsageState::Final => {}
        }
        if let Some(u) = &turn.usage {
            self.input.add(u.input_tokens);
            self.output.add(u.output_tokens);
            self.total.add(u.total_tokens);
            self.read.add(u.cache.read_input_tokens);
            self.write.add(u.cache.write_input_tokens);
            self.reasoning.add(u.output_details.reasoning_tokens);
            if let (Some(input), Some(read)) = (u.input_tokens, u.cache.read_input_tokens)
                && read <= input
            {
                self.hit_input += u128::from(input);
                self.hit_read += u128::from(read);
                self.hit_reported += 1;
            }
        }
    }
    /// 分子分母使用相同已报告轮次，不平均逐轮百分比。
    pub fn hit_rate(&self) -> Option<f64> {
        (self.hit_input > 0).then(|| self.hit_read as f64 / self.hit_input as f64 * 100.0)
    }
}
/// 实际模型分组；同名模型在不同 Provider 下分开统计。
pub struct ModelTotals {
    pub provider_name: String,
    pub upstream_model: String,
    pub totals: Totals,
}
/// 会话合计与各模型合计。
pub struct Summary {
    pub totals: Totals,
    pub models: Vec<ModelTotals>,
}
impl Summary {
    /// 同一实际模型的不同客户端别名归入同一分组。
    pub fn from_turns(turns: &[Turn]) -> Self {
        let mut totals = Totals::default();
        let mut groups = BTreeMap::<(i64, String), ModelTotals>::new();
        for turn in turns {
            totals.add(turn);
            if let Some(actual) = &turn.actual {
                let entry = groups
                    .entry((actual.provider_id, actual.upstream_model.clone()))
                    .or_insert_with(|| ModelTotals {
                        provider_name: actual.provider_name.clone(),
                        upstream_model: actual.upstream_model.clone(),
                        totals: Totals::default(),
                    });
                entry.totals.add(turn);
            }
        }
        Self {
            totals,
            models: groups.into_values().collect(),
        }
    }
}
