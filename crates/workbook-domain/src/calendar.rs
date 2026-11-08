use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

pub const LEARN_AHEAD_LIMIT_MINUTES: i64 = 20;
pub const DAY_BOUNDARY_HOUR: u32 = 4;

#[derive(Clone, Debug)]
pub struct StudyCalendar {
    timezone_name: String,
    timezone: Tz,
}

impl StudyCalendar {
    pub fn new(timezone_name: &str) -> Result<Self, String> {
        let timezone = timezone_name
            .parse::<Tz>()
            .map_err(|_| format!("不明なタイムゾーンです: {timezone_name}"))?;
        Ok(Self {
            timezone_name: timezone_name.to_string(),
            timezone,
        })
    }

    pub fn system() -> Self {
        let name = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());
        Self::new(&name).unwrap_or_else(|_| Self {
            timezone_name: "UTC".to_string(),
            timezone: Tz::UTC,
        })
    }

    pub fn timezone_name(&self) -> &str {
        &self.timezone_name
    }

    pub fn timezone(&self) -> Tz {
        self.timezone
    }

    pub fn learning_day(&self, now: DateTime<Utc>) -> NaiveDate {
        learning_day(now, self.timezone)
    }

    pub fn next_learning_day_start(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        next_learning_day_start(now, self.timezone)
    }

    pub fn day_start(&self, day: NaiveDate) -> DateTime<Utc> {
        day_start(day, self.timezone)
    }

    pub fn same_learning_day(&self, a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
        self.learning_day(a) == self.learning_day(b)
    }

    /// Learn Ahead is allowed only when the pending short-term card is due
    /// within 20 minutes *and* still belongs to the current learning day.
    pub fn learn_ahead_allowed(&self, now: DateTime<Utc>, due_at: DateTime<Utc>) -> bool {
        due_at > now
            && due_at <= now + Duration::minutes(LEARN_AHEAD_LIMIT_MINUTES)
            && self.same_learning_day(now, due_at)
    }
}

pub fn learning_day(now: DateTime<Utc>, timezone: Tz) -> NaiveDate {
    let local = now.with_timezone(&timezone).naive_local();
    (local - Duration::hours(DAY_BOUNDARY_HOUR as i64)).date()
}

pub fn next_learning_day_start(now: DateTime<Utc>, timezone: Tz) -> DateTime<Utc> {
    let local = now.with_timezone(&timezone).naive_local();
    let mut day = local.date();
    let boundary = NaiveTime::from_hms_opt(DAY_BOUNDARY_HOUR, 0, 0).expect("valid time");
    if local.time() >= boundary {
        day += Duration::days(1);
    }
    day_start(day, timezone)
}

pub fn day_start(day: NaiveDate, timezone: Tz) -> DateTime<Utc> {
    let local = day
        .and_hms_opt(DAY_BOUNDARY_HOUR, 0, 0)
        .expect("valid date");
    // A 4 a.m. wall-clock time can be skipped or repeated around DST in a few
    // zones. Prefer the earliest valid instant, then fall back to adding the
    // offset that makes 4 a.m. exist.
    match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) => value.with_timezone(&Utc),
        LocalResult::Ambiguous(first, _) => first.with_timezone(&Utc),
        LocalResult::None => {
            let candidate = local + Duration::hours(1);
            timezone
                .from_local_datetime(&candidate)
                .earliest()
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or_else(|| DateTime::<Utc>::from_naive_utc_and_offset(local, Utc))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn study_day_changes_at_four_am_local_time() {
        let tokyo = StudyCalendar::new("Asia/Tokyo").unwrap();
        assert_eq!(
            tokyo.learning_day(utc(2026, 2, 1, 18, 59)),
            NaiveDate::from_ymd_opt(2026, 2, 1).unwrap()
        );
        assert_eq!(
            tokyo.learning_day(utc(2026, 2, 1, 19, 0)),
            NaiveDate::from_ymd_opt(2026, 2, 2).unwrap()
        );
    }

    #[test]
    fn next_day_start_is_the_next_local_four_am() {
        let tokyo = StudyCalendar::new("Asia/Tokyo").unwrap();
        let now = utc(2026, 2, 2, 3, 55); // 12:55 JST Feb 2
        assert_eq!(
            tokyo.next_learning_day_start(now),
            utc(2026, 2, 2, 19, 0) // Feb 3 04:00 JST
        );
    }

    #[test]
    fn learn_ahead_respects_the_twenty_minute_boundary() {
        let tokyo = StudyCalendar::new("Asia/Tokyo").unwrap();
        let now = utc(2026, 2, 2, 3, 0); // 12:00 JST
        assert!(
            tokyo.learn_ahead_allowed(now, now + Duration::minutes(19) + Duration::seconds(59))
        );
        assert!(tokyo.learn_ahead_allowed(now, now + Duration::minutes(20)));
        assert!(
            !tokyo.learn_ahead_allowed(now, now + Duration::minutes(20) + Duration::seconds(1))
        );
    }

    #[test]
    fn learn_ahead_does_not_cross_the_learning_day() {
        let tokyo = StudyCalendar::new("Asia/Tokyo").unwrap();
        let now = utc(2026, 2, 1, 18, 55); // 03:55 JST Feb 2
        let due = utc(2026, 2, 1, 19, 5); // 04:05 JST Feb 2
        assert!(!tokyo.same_learning_day(now, due));
        assert!(!tokyo.learn_ahead_allowed(now, due));
    }

    #[test]
    fn midnight_crossing_stays_in_the_same_learning_day() {
        let tokyo = StudyCalendar::new("Asia/Tokyo").unwrap();
        let a = utc(2026, 2, 1, 14, 59); // 23:59 JST Feb 1
        let b = utc(2026, 2, 1, 15, 1); // 00:01 JST Feb 2
        assert!(tokyo.same_learning_day(a, b));
    }
}
