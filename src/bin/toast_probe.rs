//! Dev probe: dump recent toast payloads (structure only) to inspect what
//! metadata Zen/WhatsApp toasts actually carry. Not part of the app runtime.
fn main() {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let db = std::path::Path::new(&local)
        .join("Microsoft")
        .join("Windows")
        .join("Notifications")
        .join("wpndatabase.db");
    let tmp = std::env::temp_dir().join("wpn_probe_copy.db");
    std::fs::copy(&db, &tmp).expect("copy db");
    let conn = rusqlite::Connection::open_with_flags(
        &tmp,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open");
    let mut handlers = std::collections::HashMap::new();
    if let Ok(mut st) = conn.prepare("SELECT RecordId, PrimaryId FROM NotificationHandler") {
        if let Ok(rows) = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        }) {
            for (id, name) in rows.flatten() {
                handlers.insert(id, name);
            }
        }
    }
    let mut st = conn
        .prepare(
            "SELECT Id, HandlerId, CAST(ArrivalTime AS TEXT), CAST(Payload AS TEXT) \
             FROM Notification WHERE Type='toast' ORDER BY ArrivalTime DESC LIMIT 12",
        )
        .expect("query");
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .expect("rows");
    let mut out = String::new();
    for (id, hid, at, p) in rows.flatten() {
        let app = handlers.get(&hid).cloned().unwrap_or_default();
        out.push_str(&format!("=== id={} app={} at={}\n{}\n\n", id, app, at, p));
    }
    let dest = std::env::temp_dir().join("wpn_probe.txt");
    std::fs::write(&dest, out).expect("write");
    println!("written {:?}", dest);
}
