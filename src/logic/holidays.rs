use chrono::{Datelike, NaiveDate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NationalHoliday {
    pub name: &'static str,
    pub is_cuti: bool,
}

const SPECIFIC_HOLIDAYS: &[(i32, u32, u32, &str, bool)] = &[
    // 2025 (SKB 3 Menteri RI)
    (2025, 1, 27, "Isra Mikraj Nabi Muhammad SAW", false),
    (2025, 1, 28, "Cuti Bersama Tahun Baru Imlek", true),
    (2025, 1, 29, "Tahun Baru Imlek 2576 Kongzili", false),
    (2025, 3, 28, "Cuti Bersama Hari Suci Nyepi", true),
    (2025, 3, 29, "Hari Suci Nyepi 1947 Saka", false),
    (2025, 3, 31, "Hari Raya Idul Fitri 1446 H", false),
    (2025, 4, 1, "Hari Raya Idul Fitri 1446 H", false),
    (2025, 4, 2, "Cuti Bersama Idul Fitri", true),
    (2025, 4, 3, "Cuti Bersama Idul Fitri", true),
    (2025, 4, 4, "Cuti Bersama Idul Fitri", true),
    (2025, 4, 7, "Cuti Bersama Idul Fitri", true),
    (2025, 4, 18, "Wafat Yesus Kristus", false),
    (2025, 4, 20, "Kebangkitan Yesus Kristus (Paskah)", false),
    (2025, 5, 12, "Hari Raya Waisak 2569 BE", false),
    (2025, 5, 13, "Cuti Bersama Hari Raya Waisak", true),
    (2025, 5, 29, "Kenaikan Yesus Kristus", false),
    (2025, 5, 30, "Cuti Bersama Kenaikan Yesus Kristus", true),
    (2025, 6, 6, "Hari Raya Idul Adha 1446 H", false),
    (2025, 6, 9, "Cuti Bersama Idul Adha", true),
    (2025, 6, 27, "Tahun Baru Islam 1447 H", false),
    (2025, 9, 5, "Maulid Nabi Muhammad SAW", false),
    (2025, 12, 26, "Cuti Bersama Kelahiran Yesus Kristus", true),
    // 2026 (SKB 3 Menteri RI)
    (2026, 1, 16, "Isra Mikraj Nabi Muhammad SAW", false),
    (2026, 2, 16, "Cuti Bersama Tahun Baru Imlek", true),
    (2026, 2, 17, "Tahun Baru Imlek 2577 Kongzili", false),
    (2026, 3, 18, "Cuti Bersama Hari Suci Nyepi", true),
    (2026, 3, 19, "Hari Suci Nyepi 1948 Saka", false),
    (2026, 3, 20, "Cuti Bersama Idul Fitri", true),
    (2026, 3, 21, "Hari Raya Idul Fitri 1447 H", false),
    (2026, 3, 22, "Hari Raya Idul Fitri 1447 H", false),
    (2026, 3, 23, "Cuti Bersama Idul Fitri", true),
    (2026, 3, 24, "Cuti Bersama Idul Fitri", true),
    (2026, 4, 3, "Wafat Yesus Kristus", false),
    (2026, 4, 5, "Kebangkitan Yesus Kristus (Paskah)", false),
    (2026, 5, 14, "Kenaikan Yesus Kristus", false),
    (2026, 5, 15, "Cuti Bersama Kenaikan Yesus Kristus", true),
    (2026, 5, 27, "Hari Raya Idul Adha 1447 H", false),
    (2026, 5, 28, "Cuti Bersama Idul Adha", true),
    (2026, 5, 31, "Hari Raya Waisak 2570 BE", false),
    (2026, 6, 16, "Tahun Baru Islam 1448 H", false),
    (2026, 8, 25, "Maulid Nabi Muhammad SAW", false),
    (2026, 12, 24, "Cuti Bersama Kelahiran Yesus Kristus", true),
    // 2027
    (2027, 1, 5, "Isra Mikraj Nabi Muhammad SAW", false),
    (2027, 2, 6, "Tahun Baru Imlek 2578 Kongzili", false),
    (2027, 3, 9, "Hari Suci Nyepi 1949 Saka", false),
    (2027, 3, 10, "Hari Raya Idul Fitri 1448 H", false),
    (2027, 3, 11, "Hari Raya Idul Fitri 1448 H", false),
    (2027, 3, 26, "Wafat Yesus Kristus", false),
    (2027, 3, 28, "Kebangkitan Yesus Kristus (Paskah)", false),
    (2027, 5, 6, "Kenaikan Yesus Kristus", false),
    (2027, 5, 17, "Hari Raya Idul Adha 1448 H", false),
    (2027, 5, 20, "Hari Raya Waisak 2571 BE", false),
    (2027, 6, 6, "Tahun Baru Islam 1449 H", false),
    (2027, 8, 15, "Maulid Nabi Muhammad SAW", false),
    (2027, 12, 26, "Isra Mikraj Nabi Muhammad SAW", false),
];

pub fn holiday_for(date: NaiveDate) -> Option<NationalHoliday> {
    let y = date.year();
    let m = date.month();
    let d = date.day();

    match (m, d) {
        (1, 1) => {
            return Some(NationalHoliday {
                name: "Tahun Baru Masehi",
                is_cuti: false,
            })
        }
        (5, 1) => {
            return Some(NationalHoliday {
                name: "Hari Buruh Internasional",
                is_cuti: false,
            })
        }
        (6, 1) => {
            return Some(NationalHoliday {
                name: "Hari Lahir Pancasila",
                is_cuti: false,
            })
        }
        (8, 17) => {
            return Some(NationalHoliday {
                name: "Hari Kemerdekaan RI",
                is_cuti: false,
            })
        }
        (12, 25) => {
            return Some(NationalHoliday {
                name: "Hari Raya Natal",
                is_cuti: false,
            })
        }
        _ => {}
    }

    for &(hy, hm, hd, name, is_cuti) in SPECIFIC_HOLIDAYS {
        if hy == y && hm == m && hd == d {
            return Some(NationalHoliday { name, is_cuti });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::holiday_for;
    use chrono::NaiveDate;

    #[test]
    fn recognizes_fixed_and_lunar_indonesian_holidays() {
        let independence = holiday_for(NaiveDate::from_ymd_opt(2026, 8, 17).unwrap()).unwrap();
        assert_eq!(independence.name, "Hari Kemerdekaan RI");
        assert!(!independence.is_cuti);

        let future_ny = holiday_for(NaiveDate::from_ymd_opt(2030, 1, 1).unwrap()).unwrap();
        assert_eq!(future_ny.name, "Tahun Baru Masehi");

        let eid = holiday_for(NaiveDate::from_ymd_opt(2026, 3, 21).unwrap()).unwrap();
        assert!(eid.name.contains("Idul Fitri"));
        assert!(!eid.is_cuti);

        let waisak = holiday_for(NaiveDate::from_ymd_opt(2026, 5, 31).unwrap()).unwrap();
        assert!(waisak.name.contains("Waisak"));

        let cuti = holiday_for(NaiveDate::from_ymd_opt(2026, 12, 24).unwrap()).unwrap();
        assert!(cuti.is_cuti);

        assert!(holiday_for(NaiveDate::from_ymd_opt(2026, 11, 23).unwrap()).is_none());
    }
}
