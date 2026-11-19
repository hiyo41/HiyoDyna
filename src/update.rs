#![allow(dead_code)] // Wired into the message loop in Task 6.

use crate::settings::UpdateChannel;

#[derive(Clone, Debug)]
struct Release {
    tag: String,
    published: String,
}

fn repo_slug() -> &'static str {
    env!("CARGO_PKG_REPOSITORY")
        .strip_prefix("https://github.com/")
        .unwrap_or(env!("CARGO_PKG_REPOSITORY"))
        .trim_end_matches('/')
}

fn feed_url() -> String {
    format!("https://github.com/{}/releases.atom", repo_slug())
}

fn latest_release_url() -> String {
    format!(
        "https://api.github.com/repos/{}/releases/latest",
        repo_slug()
    )
}

fn parse_atom(text: &str) -> Vec<Release> {
    text.split("<entry>")
        .skip(1)
        .filter_map(|entry| {
            let tag = entry
                .split_once("/releases/tag/")
                .and_then(|(_, rest)| rest.split('"').next())
                .map(str::trim)
                .filter(|tag| !tag.is_empty())?;
            let published = between(entry, "<updated>", "</updated>")?;
            Some(Release {
                tag: tag.to_string(),
                published: published.to_string(),
            })
        })
        .collect()
}

fn parse_latest_json(text: &str) -> Result<Release, String> {
    let tag = json_string(text, "tag_name").ok_or("release JSON has no tag_name")?;
    let published = json_string(text, "published_at").ok_or("release JSON has no published_at")?;
    Ok(Release { tag, published })
}

fn select_from_atom(entries: &[Release], channel: UpdateChannel) -> Option<Release> {
    match channel {
        UpdateChannel::Build => entries
            .iter()
            .find(|entry| is_pre_release(&entry.tag))
            .cloned(),
        UpdateChannel::Stable => entries
            .iter()
            .find(|entry| entry.tag.starts_with('v'))
            .or_else(|| entries.iter().find(|entry| is_pre_release(&entry.tag)))
            .cloned(),
    }
}

fn stable_candidate(latest: Option<Release>, atom: &[Release]) -> Option<Release> {
    latest.or_else(|| select_from_atom(atom, UpdateChannel::Build))
}

fn parse_rfc3339(text: &str) -> Option<i64> {
    let (date, rest) = text.trim().split_once(['T', 't'])?;
    let (time, offset) = split_offset(rest)?;
    let (whole, fraction) = match time.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (time, None),
    };
    if let Some(fraction) = fraction
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }

    let mut date_parts = date.split('-');
    let year = number(date_parts.next()?)?;
    let month = number(date_parts.next()?)?;
    let day = number(date_parts.next()?)?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }

    let mut time_parts = whole.split(':');
    let hour = number(time_parts.next()?)?;
    let minute = number(time_parts.next()?)?;
    let second = number(time_parts.next()?)?;
    if time_parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset)
}

fn should_apply(
    release: &Release,
    applied_tag: Option<&str>,
    applied_time: Option<i64>,
    installed_time: Option<i64>,
    embedded_tag: Option<&str>,
) -> bool {
    if applied_tag == Some(release.tag.as_str()) || embedded_tag == Some(release.tag.as_str()) {
        return false;
    }
    let Some(published) = parse_rfc3339(&release.published) else {
        return false;
    };
    match applied_time.or(installed_time) {
        Some(installed) => published > installed,
        None => true,
    }
}

fn select_release(channel: UpdateChannel) -> Result<Option<Release>, String> {
    match channel {
        UpdateChannel::Build => Ok(select_from_atom(&fetch_atom()?, channel)),
        UpdateChannel::Stable => match crate::net::get(&latest_release_url(), 1024 * 1024) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes)
                    .map_err(|error| format!("release JSON is not UTF-8: {error}"))?;
                Ok(Some(parse_latest_json(&text)?))
            }
            Err(error) if error.status == Some(404) => Ok(stable_candidate(None, &fetch_atom()?)),
            Err(error) => Err(error.to_string()),
        },
    }
}

fn fetch_atom() -> Result<Vec<Release>, String> {
    let bytes = crate::net::get(&feed_url(), 1024 * 1024).map_err(|error| error.to_string())?;
    let text =
        String::from_utf8(bytes).map_err(|error| format!("release feed is not UTF-8: {error}"))?;
    Ok(parse_atom(&text))
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let (_, rest) = text.split_once(start)?;
    let (value, _) = rest.split_once(end)?;
    Some(value.trim())
}

fn json_string(text: &str, key: &str) -> Option<String> {
    let (_, rest) = text.split_once(&format!("\"{key}\""))?;
    let (_, rest) = rest.split_once(':')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    let (value, _) = rest.split_once('"')?;
    Some(value.to_string())
}

fn is_pre_release(tag: &str) -> bool {
    tag.strip_prefix("pre-").is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn split_offset(text: &str) -> Option<(&str, i64)> {
    if let Some(time) = text.strip_suffix(['Z', 'z']) {
        return Some((time, 0));
    }
    let index = text.find(['+', '-'])?;
    let (time, zone) = text.split_at(index);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let (hours, minutes) = zone[1..].split_once(':')?;
    let hours = number(hours)?;
    let minutes = number(minutes)?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((time, sign * (hours * 3_600 + minutes * 60)))
}

fn number(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use crate::settings::UpdateChannel;

    const ATOM: &str = r#"<feed>
<entry><updated>2026-11-19T08:07:44Z</updated>
<link rel="alternate" href="https://github.com/hiyo41/HiyoDyna/releases/tag/pre-2"/></entry>
<entry><updated>2026-11-18T08:07:44Z</updated>
<link rel="alternate" href="https://github.com/hiyo41/HiyoDyna/releases/tag/pre-1"/></entry>
</feed>"#;

    #[test]
    fn parses_atom_entries() {
        let entries = super::parse_atom(ATOM);
        assert_eq!(entries[0].tag, "pre-2");
        assert_eq!(entries[0].published, "2026-11-19T08:07:44Z");
        assert_eq!(entries[1].tag, "pre-1");
    }

    #[test]
    fn parses_latest_release_json() {
        let release = super::parse_latest_json(
            r#"{"tag_name":"v0.0.1-3","published_at":"2026-11-19T09:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(release.tag, "v0.0.1-3");
        assert_eq!(release.published, "2026-11-19T09:00:00Z");
    }

    #[test]
    fn build_takes_pre_release_and_stable_takes_official() {
        let entries = super::parse_atom(ATOM);
        assert_eq!(
            super::select_from_atom(&entries, UpdateChannel::Build)
                .unwrap()
                .tag,
            "pre-2"
        );
        let official = super::parse_atom(
            r#"<entry><updated>2026-11-19T10:00:00Z</updated>
        <link href="https://github.com/hiyo41/HiyoDyna/releases/tag/v0.0.1-3"/></entry>"#,
        );
        assert_eq!(
            super::select_from_atom(&official, UpdateChannel::Stable)
                .unwrap()
                .tag,
            "v0.0.1-3"
        );
        assert_eq!(
            super::select_from_atom(&entries, UpdateChannel::Stable)
                .unwrap()
                .tag,
            "pre-2"
        );
    }

    #[test]
    fn parses_rfc3339_offsets_and_epochs() {
        assert_eq!(super::parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            super::parse_rfc3339("2000-01-01T00:00:00Z"),
            Some(946684800)
        );
        assert_eq!(
            super::parse_rfc3339("2026-11-19T15:07:44+07:00"),
            super::parse_rfc3339("2026-11-19T08:07:44Z")
        );
        assert_eq!(
            super::parse_rfc3339("2026-11-19T15:26:51.0835595+07:00"),
            super::parse_rfc3339("2026-11-19T08:26:51Z")
        );
    }

    #[test]
    fn refuses_same_or_older_releases() {
        let release = super::Release {
            tag: "pre-3".into(),
            published: "2026-11-19T10:00:00Z".into(),
        };
        let installed_before = super::parse_rfc3339("2026-11-19T09:00:00Z");
        let installed_after = super::parse_rfc3339("2026-11-19T11:00:00Z");
        assert!(!super::should_apply(
            &release,
            Some("pre-3"),
            None,
            None,
            None
        ));
        assert!(!super::should_apply(
            &release,
            None,
            None,
            installed_after,
            None
        ));
        assert!(super::should_apply(
            &release,
            None,
            None,
            installed_before,
            None
        ));
        assert!(!super::should_apply(
            &release,
            None,
            None,
            None,
            Some("pre-3")
        ));
    }

    #[test]
    fn stable_uses_official_then_falls_back() {
        let atom = super::parse_atom(ATOM);
        let official = super::Release {
            tag: "v0.0.1-3".into(),
            published: "2026-11-19T10:00:00Z".into(),
        };
        assert_eq!(
            super::stable_candidate(Some(official), &atom).unwrap().tag,
            "v0.0.1-3"
        );
        assert_eq!(super::stable_candidate(None, &atom).unwrap().tag, "pre-2");
    }
}
