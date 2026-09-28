use std::{sync::LazyLock, time::Duration};

use chrono::NaiveDate;
use llmproxy_store::{HolidayDate, HolidayKind};
use regex::Regex;

/// Primary source: the State Council General Office notice on the China Government website.
pub const SOURCE_2026: &str = "https://www.gov.cn/zhengce/zhengceku/202511/content_7047091.htm";

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{1,2})月(\d{1,2})日[^。]*?至(?:(\d{1,2})月)?(\d{1,2})日").unwrap()
});
static DAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{1,2})月(\d{1,2})日").unwrap());
static COUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"共(\d+)天").unwrap());

const HOLIDAYS: [(&str, &str); 7] = [
    ("一", "元旦"),
    ("二", "春节"),
    ("三", "清明节"),
    ("四", "劳动节"),
    ("五", "端午节"),
    ("六", "中秋节"),
    ("七", "国庆节"),
];

pub async fn fetch_2026() -> Result<Vec<HolidayDate>, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|error| format!("无法建立官方来源连接：{error}"))?;
    let response = client
        .get(SOURCE_2026)
        .send()
        .await
        .map_err(|error| format!("获取国务院通知失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("国务院通知返回 HTTP {}", response.status()));
    }
    let html = response
        .bytes()
        .await
        .map_err(|error| format!("读取国务院通知失败：{error}"))?;
    if html.len() > 512 * 1024 {
        return Err("国务院通知页面过大，已停止导入".into());
    }
    let html = std::str::from_utf8(&html).map_err(|_| "国务院通知不是 UTF-8 文本")?;
    parse_2026(html)
}

pub fn parse_2026(html: &str) -> Result<Vec<HolidayDate>, String> {
    let content = html
        .split_once("id=\"UCAP-CONTENT\"")
        .and_then(|(_, content)| content.split_once("id=\"pagination\""))
        .map(|(content, _)| content)
        .ok_or("未找到国务院通知正文，已停止导入")?;
    let content = content.replace("</p>", "\n");
    let text = TAG.replace_all(&content, "");
    if !text.contains("国务院办公厅关于2026年")
        || !text.contains("部分节假日安排的通知")
        || !text.contains("国办发明电〔2025〕7号")
    {
        return Err("官方通知标题或文号不符，已停止导入".into());
    }
    let mut dates = Vec::new();
    for (number, name) in HOLIDAYS {
        let heading = format!("{number}、{name}：");
        let line = text
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with(&heading))
            .ok_or_else(|| format!("未找到{name}安排，已停止导入"))?;
        let detail = line.strip_prefix(&heading).unwrap();
        let (vacation, after) = detail
            .split_once('。')
            .ok_or_else(|| format!("{name}安排不完整，已停止导入"))?;
        if !vacation.contains("放假") {
            return Err(format!("{name}放假日期未识别，已停止导入"));
        }
        let range = RANGE
            .captures(vacation)
            .ok_or_else(|| format!("{name}放假区间未识别，已停止导入"))?;
        let start_month = number_at(&range, 1)?;
        let start = date_2026(start_month, number_at(&range, 2)?)?;
        let end = date_2026(
            range.get(3).map_or(Ok(start_month), |part| {
                part.as_str()
                    .parse::<u32>()
                    .map_err(|_| "通知日期无效".to_owned())
            })?,
            number_at(&range, 4)?,
        )?;
        let declared = COUNT
            .captures(vacation)
            .ok_or_else(|| format!("{name}放假天数未识别，已停止导入"))?;
        let declared = number_at(&declared, 1)? as i64;
        let actual = (end - start).num_days() + 1;
        if !(1..=15).contains(&actual) || actual != declared {
            return Err(format!("{name}放假区间与通知天数不一致，已停止导入"));
        }
        for offset in 0..actual {
            dates.push(HolidayDate {
                date: (start + chrono::Duration::days(offset)).to_string(),
                name: name.into(),
                kind: HolidayKind::Holiday,
                source_url: SOURCE_2026.into(),
            });
        }
        let work = after.split('。').next().unwrap_or("");
        if !work.is_empty() {
            if !work.contains("上班") {
                return Err(format!("{name}调休安排未识别，已停止导入"));
            }
            let mut found = false;
            for date in DAY.captures_iter(work) {
                found = true;
                dates.push(HolidayDate {
                    date: date_2026(number_at(&date, 1)?, number_at(&date, 2)?)?.to_string(),
                    name: format!("{name}调休上班"),
                    kind: HolidayKind::AdjustedWorkday,
                    source_url: SOURCE_2026.into(),
                });
            }
            if !found {
                return Err(format!("{name}调休日期未识别，已停止导入"));
            }
        }
    }
    dates.sort_by(|a, b| a.date.cmp(&b.date));
    if dates.windows(2).any(|pair| pair[0].date == pair[1].date) {
        return Err("国务院通知中出现重复日期，已停止导入".into());
    }
    Ok(dates)
}

fn number_at(captures: &regex::Captures<'_>, index: usize) -> Result<u32, String> {
    captures[index]
        .parse()
        .map_err(|_| "通知日期无效，已停止导入".into())
}

fn date_2026(month: u32, day: u32) -> Result<NaiveDate, String> {
    NaiveDate::from_ymd_opt(2026, month, day).ok_or_else(|| "通知日期无效，已停止导入".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTICE: &str = r#"<div id="UCAP-CONTENT"><p>国务院办公厅关于2026年</p><p>部分节假日安排的通知</p><p>国办发明电〔2025〕7号</p>
<p><strong>一、元旦：</strong>1月1日（周四）至3日（周六）放假调休，共3天。1月4日（周日）上班。</p>
<p><strong>二、春节：</strong>2月15日（农历腊月二十八、周日）至23日（农历正月初七、周一）放假调休，共9天。2月14日（周六）、2月28日（周六）上班。</p>
<p><strong>三、清明节：</strong>4月4日（周六）至6日（周一）放假，共3天。</p>
<p><strong>四、劳动节：</strong>5月1日（周五）至5日（周二）放假调休，共5天。5月9日（周六）上班。</p>
<p><strong>五、端午节：</strong>6月19日（周五）至21日（周日）放假，共3天。</p>
<p><strong>六、中秋节：</strong>9月25日（周五）至27日（周日）放假，共3天。</p>
<p><strong>七、国庆节：</strong>10月1日（周四）至7日（周三）放假调休，共7天。9月20日（周日）、10月10日（周六）上班。</p>
</div><div id="pagination"></div>"#;

    #[test]
    fn imports_full_vacation_intervals_and_adjusted_workdays() {
        let dates = parse_2026(NOTICE).unwrap();
        assert_eq!(dates.len(), 39);
        assert!(
            dates
                .iter()
                .any(|d| d.date == "2026-02-23" && d.kind == HolidayKind::Holiday)
        );
        assert!(
            dates
                .iter()
                .any(|d| d.date == "2026-02-28" && d.kind == HolidayKind::AdjustedWorkday)
        );
    }

    #[test]
    fn rejects_incomplete_official_notice() {
        assert!(parse_2026(&NOTICE.replace("七、国庆节", "七、其他")).is_err());
        assert!(parse_2026(&NOTICE.replace("共9天", "共8天")).is_err());
    }
}
