//! Перенос данных: закладки и пароли из Chrome и Edge, зашифрованный файл настроек.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce as GcmNonce};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

use crate::store;

pub const BM_FILE: &str = "petal_bookmarks.json";

// ───────────── Закладки ─────────────

/// Закладка: название и адрес.
pub type Bookmark = (String, String);

fn walk_bookmarks(v: &serde_json::Value, out: &mut Vec<Bookmark>) {
    if let Some(o) = v.as_object() {
        if o.get("type").and_then(|t| t.as_str()) == Some("url") {
            let name = o.get("name").and_then(|x| x.as_str()).unwrap_or("");
            if let Some(url) = o.get("url").and_then(|x| x.as_str()) {
                push_bookmark(out, name, url);
            }
        }
        for key in ["children", "roots"] {
            if let Some(c) = o.get(key) {
                walk_bookmarks(c, out);
            }
        }
        // В «roots» Chrome кладёт bookmark_bar, other и synced как отдельные поля.
        if o.contains_key("bookmark_bar") || o.contains_key("other") || o.contains_key("synced") {
            for (_, x) in o.iter().filter(|(_, x)| x.is_object()) {
                walk_bookmarks(x, out);
            }
        }
    } else if let Some(a) = v.as_array() {
        for x in a {
            walk_bookmarks(x, out);
        }
    }
}

fn push_bookmark(out: &mut Vec<Bookmark>, name: &str, url: &str) {
    let url = url.trim();
    let lower = url.to_lowercase();
    if (lower.starts_with("http://") || lower.starts_with("https://")) && url.len() <= 2000 {
        out.push((name.trim().chars().take(80).collect(), url.to_string()));
    }
}

/// Читает файл закладок Chrome или Edge (JSON `Bookmarks`).
pub fn parse_browser_bookmarks(json: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
        walk_bookmarks(&v, &mut out);
    }
    out
}

/// Добавляет новые закладки к списку без повторов по адресу; возвращает, сколько добавлено.
pub fn merge_bookmarks(list: &mut Vec<Bookmark>, new: Vec<Bookmark>) -> usize {
    let mut added = 0;
    for b in new {
        if !list.iter().any(|x| x.1 == b.1) {
            list.push(b);
            added += 1;
        }
    }
    added
}

pub fn bookmarks_to_json(list: &[Bookmark]) -> String {
    serde_json::Value::Array(list.iter().map(|(n, u)| serde_json::json!({ "n": n, "u": u })).collect()).to_string()
}

/// Читает наш список закладок (массив `{n,u}`); лишнее и неверное отбрасывает.
pub fn bookmarks_from_json(text: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    if let Ok(serde_json::Value::Array(a)) = serde_json::from_str::<serde_json::Value>(text) {
        for x in a {
            let (n, u) = (x.get("n").and_then(|v| v.as_str()).unwrap_or(""), x.get("u").and_then(|v| v.as_str()).unwrap_or(""));
            push_bookmark(&mut out, n, u);
        }
    }
    out
}

// ───────────── Профили Chrome и Edge ─────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Browser {
    Chrome,
    Edge,
}

impl Browser {
    pub fn name(self) -> &'static str {
        match self {
            Browser::Chrome => "Chrome",
            Browser::Edge => "Edge",
        }
    }

    fn user_data(self) -> Option<PathBuf> {
        let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
        Some(match self {
            Browser::Chrome => base.join("Google").join("Chrome").join("User Data"),
            Browser::Edge => base.join("Microsoft").join("Edge").join("User Data"),
        })
    }
}

/// Папки профилей: `Default` и `Profile 1`, `Profile 2`…
fn profiles(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let d = root.join("Default");
    if d.is_dir() {
        v.push(d);
    }
    if let Ok(rd) = std::fs::read_dir(root) {
        let mut extra: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Profile ")))
            .collect();
        extra.sort();
        v.extend(extra);
    }
    v
}

/// Закладки всех профилей браузера.
pub fn read_browser_bookmarks(b: Browser) -> Result<Vec<Bookmark>, String> {
    let root = b.user_data().filter(|r| r.is_dir()).ok_or_else(|| format!("{} на этом компьютере не найден.", b.name()))?;
    let mut all = Vec::new();
    for p in profiles(&root) {
        if let Ok(text) = std::fs::read_to_string(p.join("Bookmarks")) {
            let found = parse_browser_bookmarks(&text);
            merge_bookmarks(&mut all, found);
        }
    }
    if all.is_empty() {
        return Err(format!("В {} не нашлось закладок.", b.name()));
    }
    Ok(all)
}

// ───────────── Пароли ─────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Login {
    pub url: String,
    pub user: String,
    pub password: String,
}

pub struct PwReport {
    pub logins: Vec<Login>,
    /// Записи, которые не удалось расшифровать (например, защита нового Chrome).
    pub skipped: usize,
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim().bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(windows)]
fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    // Ключ Chrome и Edge защищён Windows для текущего пользователя, снять защиту может только он сам.
    unsafe {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out);
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Some(v)
    }
}

#[cfg(not(windows))]
fn dpapi_unprotect(_: &[u8]) -> Option<Vec<u8>> {
    None
}

/// Достаёт из `Local State` ключ, которым браузер шифрует пароли.
fn master_key(local_state: &Path) -> Result<[u8; 32], String> {
    let text = std::fs::read_to_string(local_state).map_err(|_| "Не найден файл Local State браузера.".to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| "Local State повреждён.".to_string())?;
    let enc = v.pointer("/os_crypt/encrypted_key").and_then(|x| x.as_str()).ok_or("В Local State нет ключа паролей.")?;
    let raw = b64_decode(enc).ok_or("Ключ паролей записан в неизвестном виде.")?;
    let body = raw.strip_prefix(b"DPAPI").ok_or("Ключ паролей защищён способом, который Petal не поддерживает.")?;
    let key = dpapi_unprotect(body).ok_or("Windows не сняла защиту с ключа паролей.")?;
    key.try_into().map_err(|_| "Ключ паролей неверной длины.".to_string())
}

/// Расшифровывает пароль формата `v10`/`v11` (AES-256-GCM). `v20` и прочее не поддерживается.
fn decrypt_password(key: &[u8; 32], blob: &[u8]) -> Option<String> {
    if blob.len() < 3 + 12 + 16 || !(blob.starts_with(b"v10") || blob.starts_with(b"v11")) {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let plain = cipher.decrypt(GcmNonce::from_slice(&blob[3..15]), &blob[15..]).ok()?;
    String::from_utf8(plain).ok()
}

/// Читает таблицу `logins` и расшифровывает пароли. Пустые адреса и пароли пропускаются.
fn read_logins(db: &Path, key: &[u8; 32]) -> Result<(Vec<Login>, usize), String> {
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    let mut st = conn.prepare("SELECT origin_url, username_value, password_value FROM logins").map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?)))
        .map_err(|e| e.to_string())?;
    let (mut out, mut skipped) = (Vec::new(), 0);
    for row in rows.flatten() {
        if row.0.is_empty() || row.2.is_empty() {
            continue;
        }
        match decrypt_password(key, &row.2) {
            Some(p) if !p.is_empty() => out.push(Login { url: row.0, user: row.1, password: p }),
            _ => skipped += 1,
        }
    }
    Ok((out, skipped))
}

/// Пароли всех профилей Chrome или Edge. Браузер должен быть закрыт: иначе он держит файл.
pub fn read_browser_passwords(b: Browser) -> Result<PwReport, String> {
    let root = b.user_data().filter(|r| r.is_dir()).ok_or_else(|| format!("{} на этом компьютере не найден.", b.name()))?;
    let key = master_key(&root.join("Local State"))?;
    let (mut logins, mut skipped) = (Vec::new(), 0);
    let mut found = false;
    for p in profiles(&root) {
        let db = p.join("Login Data");
        if !db.is_file() {
            continue;
        }
        found = true;
        let tmp = std::env::temp_dir().join(format!("petal_login_{}.db", std::process::id()));
        std::fs::copy(&db, &tmp).map_err(|_| format!("Не удалось прочитать пароли: закройте {} полностью и повторите.", b.name()))?;
        let r = read_logins(&tmp, &key);
        let _ = std::fs::remove_file(&tmp);
        let (l, s) = r?;
        logins.extend(l);
        skipped += s;
    }
    if !found {
        return Err(format!("В {} не нашлось сохранённых паролей.", b.name()));
    }
    Ok(PwReport { logins, skipped })
}

/// Разбирает CSV, который Chrome и Edge выгружают из своих настроек паролей (name,url,username,password).
pub fn parse_password_csv(text: &str) -> Vec<Login> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let (mut row, mut cur, mut quoted) = (Vec::<String>::new(), String::new(), false);
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => row.push(std::mem::take(&mut cur)),
            ('\n', false) | ('\r', false) => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut cur));
                rows.push(std::mem::take(&mut row));
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() || !row.is_empty() {
        row.push(cur);
        rows.push(row);
    }
    let Some(head) = rows.first() else { return Vec::new() };
    let col = |name: &str| head.iter().position(|h| h.trim().eq_ignore_ascii_case(name));
    let (Some(iu), Some(il), Some(ip)) = (col("url"), col("username"), col("password")) else { return Vec::new() };
    rows.iter()
        .skip(1)
        .filter_map(|r| {
            let (url, user, password) = (r.get(iu)?.trim(), r.get(il)?.trim(), r.get(ip)?);
            (!url.is_empty() && !password.is_empty()).then(|| Login { url: url.to_string(), user: user.to_string(), password: password.clone() })
        })
        .collect()
}

/// Красивое имя сайта для хранилища: адрес без `https://` и пути.
pub fn site_label(url: &str) -> String {
    store::url_host(url).map(|(h, _)| h.trim_start_matches("www.").to_string()).unwrap_or_else(|| url.chars().take(80).collect())
}

// ───────────── Файл настроек ─────────────

const SYNC_MAGIC: &[u8; 4] = b"PSY1";

/// Собирает файлы в один зашифрованный блок (Argon2 + ChaCha20-Poly1305, как хранилище паролей).
pub fn sync_pack(files: &[(String, Vec<u8>)], pass: &str) -> Vec<u8> {
    let mut plain = Vec::new();
    for (name, data) in files {
        plain.extend_from_slice(&(name.len() as u16).to_le_bytes());
        plain.extend_from_slice(name.as_bytes());
        plain.extend_from_slice(&(data.len() as u32).to_le_bytes());
        plain.extend_from_slice(data);
    }
    let salt = store::random_bytes::<16>();
    let nonce = store::random_bytes::<12>();
    let key = store::derive_key(pass, &salt);
    let ct = ChaCha20Poly1305::new(Key::from_slice(&key)).encrypt(Nonce::from_slice(&nonce), plain.as_slice()).expect("шифрование");
    let mut out = Vec::with_capacity(32 + ct.len());
    out.extend_from_slice(SYNC_MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

#[derive(Debug, PartialEq)]
pub enum SyncError {
    NotSyncFile,
    WrongPassword,
    Corrupt,
}

/// Расшифровывает блок; берёт только файлы из списка `allowed`, остальные игнорирует.
pub fn sync_unpack(bytes: &[u8], pass: &str, allowed: &[&str]) -> Result<Vec<(String, Vec<u8>)>, SyncError> {
    if bytes.len() < 4 + 16 + 12 + 16 || &bytes[..4] != SYNC_MAGIC {
        return Err(SyncError::NotSyncFile);
    }
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&bytes[4..20]);
    let key = store::derive_key(pass, &salt);
    let plain = ChaCha20Poly1305::new(Key::from_slice(&key)).decrypt(Nonce::from_slice(&bytes[20..32]), &bytes[32..]).map_err(|_| SyncError::WrongPassword)?;
    let (mut out, mut i) = (Vec::new(), 0usize);
    while i < plain.len() {
        let nl = u16::from_le_bytes(plain.get(i..i + 2).ok_or(SyncError::Corrupt)?.try_into().unwrap()) as usize;
        i += 2;
        let name = std::str::from_utf8(plain.get(i..i + nl).ok_or(SyncError::Corrupt)?).map_err(|_| SyncError::Corrupt)?.to_string();
        i += nl;
        let dl = u32::from_le_bytes(plain.get(i..i + 4).ok_or(SyncError::Corrupt)?.try_into().unwrap()) as usize;
        i += 4;
        let data = plain.get(i..i + dl).ok_or(SyncError::Corrupt)?.to_vec();
        i += dl;
        if allowed.contains(&name.as_str()) {
            out.push((name, data));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_bookmarks_are_flattened() {
        let j = r#"{"roots":{"bookmark_bar":{"children":[
            {"type":"url","name":"A","url":"https://a.example/"},
            {"type":"folder","name":"F","children":[{"type":"url","name":"B","url":"http://b.example/x"},{"type":"url","name":"JS","url":"javascript:alert(1)"}]}]},
            "other":{"children":[{"type":"url","name":"A2","url":"https://a.example/"},{"type":"url","name":"C","url":"https://c.example"}]},
            "synced":{"children":[]}}}"#;
        let mut list = parse_browser_bookmarks(j);
        assert_eq!(list.len(), 4);
        assert!(list.iter().all(|b| b.1.starts_with("http")));
        let added = merge_bookmarks(&mut list, vec![("X".into(), "https://c.example".into()), ("Y".into(), "https://y.example".into())]);
        assert_eq!(added, 1);
        assert_eq!(bookmarks_from_json(&bookmarks_to_json(&list)), list);
        assert!(parse_browser_bookmarks("не json").is_empty());
    }

    #[test]
    fn base64_decoder() {
        assert_eq!(b64_decode("SGVsbG8=").unwrap(), b"Hello");
        assert_eq!(b64_decode("RFBBUEk=").unwrap(), b"DPAPI");
        assert!(b64_decode("a$b").is_none());
    }

    #[test]
    fn chrome_password_blobs_decrypt_from_sqlite() {
        let key = [7u8; 32];
        let enc = |p: &str| {
            let nonce = [3u8; 12];
            let mut v = b"v10".to_vec();
            v.extend_from_slice(&nonce);
            v.extend(Aes256Gcm::new_from_slice(&key).unwrap().encrypt(GcmNonce::from_slice(&nonce), p.as_bytes()).unwrap());
            v
        };
        let path = std::env::temp_dir().join(format!("petal_test_logins_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute("CREATE TABLE logins (origin_url TEXT, username_value TEXT, password_value BLOB)", []).unwrap();
            for (u, n, p) in [("https://a.example/login", "ann", enc("секрет1")), ("https://b.example/", "bob", enc("p2")), ("https://c.example/", "cat", b"v20garbage-garbage-garbage-garbage".to_vec()), ("", "x", enc("z"))] {
                c.execute("INSERT INTO logins VALUES (?1, ?2, ?3)", rusqlite::params![u, n, p]).unwrap();
            }
        }
        let (logins, skipped) = read_logins(&path, &key).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(logins.len(), 2);
        assert_eq!(logins[0], Login { url: "https://a.example/login".into(), user: "ann".into(), password: "секрет1".into() });
        assert_eq!(skipped, 1);
        assert!(decrypt_password(&[8u8; 32], &enc("x")).is_none());
        assert_eq!(site_label("https://www.a.example/login?x=1"), "a.example");
    }

    #[test]
    fn password_csv_with_quotes_and_commas() {
        let csv = "name,url,username,password,note\r\nA,https://a.example/,ann,\"p,\"\"q\"\"\",\r\nB,https://b.example/,bob,pw2,\"многострочная\nзаметка\"\r\nC,,x,y,\r\n";
        let l = parse_password_csv(csv);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].password, "p,\"q\"");
        assert_eq!(l[1].user, "bob");
        assert!(parse_password_csv("a,b\n1,2").is_empty());
    }

    #[test]
    fn sync_file_roundtrip_and_safety() {
        let files = vec![("petal_browser.txt".to_string(), b"zoom=100".to_vec()), ("../evil.txt".to_string(), b"x".to_vec())];
        let packed = sync_pack(&files, "парольная фраза");
        let got = sync_unpack(&packed, "парольная фраза", &["petal_browser.txt"]).unwrap();
        assert_eq!(got, vec![("petal_browser.txt".to_string(), b"zoom=100".to_vec())]);
        assert_eq!(sync_unpack(&packed, "другая", &["petal_browser.txt"]), Err(SyncError::WrongPassword));
        assert_eq!(sync_unpack(b"not a sync file at all, long enough bytes..", "x", &[]), Err(SyncError::NotSyncFile));
        let mut bad = packed.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert_eq!(sync_unpack(&bad, "парольная фраза", &[]), Err(SyncError::WrongPassword));
    }
}
