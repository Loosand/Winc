use super::*;

#[test]
fn calendar_weekday_and_year_rollover() {
    assert_eq!(first_weekday(2026, 11), 0);
    assert_eq!(first_weekday(2024, 2), 4);
    assert_eq!(first_weekday(2000, 1), 6);
    assert_eq!(step_month((2026, 12), 1), (2027, 1));
    assert_eq!(step_month((2027, 1), -1), (2026, 12));
    assert_eq!(step_month((0, 1), -1), (0, 1));
}
