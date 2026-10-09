use std::ops::Range;

use topcoat::{
    Result,
    context::Cx,
    runtime::{Event, Signal, signal},
    view::{View, attributes, component, view},
};
use topcoat_ant_design::{table_page_size_select, table_pagination};

pub(super) struct Pagination {
    pub page: Signal<usize>,
    size: Signal<String>,
}

impl Pagination {
    pub fn new(cx: &Cx) -> Self {
        Self {
            page: signal(cx, || 1usize),
            size: signal(cx, || "10".to_owned()),
        }
    }

    fn size(&self) -> usize {
        match self.size.get().as_str() {
            "15" => 15,
            "30" => 30,
            "50" => 50,
            "100" => 100,
            _ => 10,
        }
    }

    pub fn range(&self, total: usize) -> Range<usize> {
        page_range(total, self.page.get(), self.size())
    }
}

fn page_range(total: usize, page: usize, size: usize) -> Range<usize> {
    let current = page.clamp(1, total.div_ceil(size).max(1));
    let start = (current - 1) * size;
    start..(start + size).min(total)
}

pub(super) fn page_numbers(current: usize, total: usize) -> Vec<usize> {
    let mut visible = vec![
        1,
        current.saturating_sub(1).max(1),
        current,
        current.saturating_add(1).min(total),
        total,
    ];
    visible.sort_unstable();
    visible.dedup();
    let mut result = Vec::new();
    for number in visible {
        if result.last().is_some_and(|previous| number > previous + 1) {
            result.push(0);
        }
        result.push(number);
    }
    result
}

#[component]
pub(super) async fn pagination(
    cx: &Cx,
    state: &Pagination,
    total: usize,
    id: &str,
    label: &str,
) -> Result<impl View> {
    let page = state.page.clone();
    let page_size = state.size.clone();
    let size = state.size();
    let range = state.range(total);
    let count = total.div_ceil(size).max(1);
    let current = range.start / size + 1;
    let previous = current.saturating_sub(1).max(1);
    let next = (current + 1).min(count);
    let pages = page_numbers(current, count);
    let summary = format!(
        "显示 {}–{} 条，共 {total} 条",
        if total == 0 { 0 } else { range.start + 1 },
        range.end
    );
    Ok(view! {
        table_pagination(summary: summary.as_str(), label: label, attrs: attributes! { class="border-t border-border [&_nav]:flex-wrap" },
            table_page_size_select(id: id, label: "每页记录数", attrs: attributes! { cx =>
                :value=$(page_size.get())
                @change=$(|event: Event| { page_size.set(event.target.value); page.set(1); })
            },
                <option value="10">"10"</option><option value="15">"15"</option><option value="30">"30"</option><option value="50">"50"</option><option value="100">"100"</option>
            )
            <button type="button" :disabled=(current == 1) @click=$(|_event: Event| page.set(previous))>"上一页"</button>
            for number in pages {
                if number == 0 { <span aria-hidden="true">"…"</span> }
                else { <button type="button" aria-label=(format!("第 {number} 页")) aria-current=(if number == current { Some("page") } else { None }) @click=$(|_event: Event| page.set(number))>(number)</button> }
            }
            <button type="button" :disabled=(current == count) @click=$(|_event: Event| page.set(next))>"下一页"</button>
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{page_numbers, page_range};

    #[test]
    fn pagination_bounds_cover_empty_lists_resize_and_removed_last_page() {
        assert_eq!(page_range(0, 1, 10), 0..0);
        assert_eq!(page_range(21, 2, 10), 10..20);
        assert_eq!(page_range(21, 3, 10), 20..21);
        assert_eq!(page_range(20, 3, 10), 10..20);
        assert_eq!(page_range(21, 1, 15), 0..15);
        assert_eq!(page_numbers(5, 10), vec![1, 0, 4, 5, 6, 0, 10]);
    }
}
