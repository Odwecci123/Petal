//! Хранение данных браузера: настройки, прошлая сессия, история и зашифрованные пароли.
//! Здесь только данные и файлы, без окон, поэтому всё проверяется тестами.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use std::time::{SystemTime, UNIX_EPOCH};

pub const BROWSER_FILE: &str = "petal_browser.txt";
pub const SESSION_FILE: &str = "petal_session.txt";
pub const HISTORY_FILE: &str = "petal_history.txt";
pub const VAULT_FILE: &str = "petal_passwords.bin";
pub const DOWNLOADS_FILE: &str = "petal_downloads.txt";

pub const MAX_HISTORY: usize = 3000;
const MAX_TABS: usize = 40;

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o
}

fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(ch) = it.next() {
        if ch != '\\' {
            o.push(ch);
            continue;
        }
        match it.next() {
            Some('t') => o.push('\t'),
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            Some('\\') => o.push('\\'),
            Some(c) => o.push(c),
            None => {}
        }
    }
    o
}

// ───────────── Где лежат данные ─────────────

/// Папка для данных программы. В `Program Files` писать нельзя, поэтому установленная версия
/// хранит всё в `%LOCALAPPDATA%\Petal`. Файл `portable.txt` рядом с программой включает
/// переносной режим: данные лежат рядом с ней, например на флешке.
pub fn pick_data_dir(
    local_app_data: Option<std::ffi::OsString>,
    exe_dir: Option<std::path::PathBuf>,
    portable: bool,
) -> Option<std::path::PathBuf> {
    if portable {
        return exe_dir;
    }
    local_app_data.filter(|v| !v.is_empty()).map(|v| std::path::PathBuf::from(v).join("Petal"))
}

// ───────────── Настройки браузера ─────────────

#[derive(Clone, PartialEq, Debug)]
pub struct BrowserCfg {
    /// При запуске открывать вкладки, которые были при закрытии.
    pub restore: bool,
    /// Записывать историю посещений.
    pub save_history: bool,
    /// Страница новой вкладки.
    pub home: String,
    /// Поисковая система: google, yandex, ddg, bing.
    pub search: String,
    /// Защищённый DNS: off, cloudflare, google, quad9, adguard, yandex, comss, custom.
    pub doh: String,
    /// Свой адрес DoH-сервера (https://…/dns-query).
    pub doh_custom: String,
    /// Прокси: пусто, либо http://хост:порт, либо socks5://хост:порт.
    pub proxy: String,
    /// Масштаб страниц в процентах.
    pub zoom: u32,
    /// Не открывать всплывающие окна сайтов.
    pub block_popups: bool,
    /// Говорить сайтам «не отслеживайте меня» (DNT и GPC).
    pub privacy_signals: bool,
    /// Стирать cookie и кэш при следующем запуске.
    pub clear_on_exit: bool,
    /// Как представляться сайтам: default, chrome, firefox, mobile.
    pub ua: String,
    /// Отключить аппаратное ускорение.
    pub no_gpu: bool,
    /// Не раскрывать настоящий IP через WebRTC.
    pub webrtc_safe: bool,
    /// Своя папка для загрузок (пусто = обычная).
    pub dl_dir: String,
    /// Язык сайтов: пусто (как в системе), ru, en.
    pub lang: String,
    /// Тёмная тема для сайтов, которые её поддерживают.
    pub dark_sites: bool,
    /// Цветовая тема оформления: id из THEMES или custom.
    pub theme: String,
    /// Свои цвета (0xRRGGBB), если выбрана тема custom.
    pub accent_a: u32,
    pub accent_b: u32,
    /// Масштаб интерфейса в процентах.
    pub ui_scale: u32,
    /// Ширина левой панели.
    pub sidebar_w: u32,
    /// Анимации интерфейса и страницы.
    pub anim: bool,
    /// Мягкое светящееся пятно за левой панелью.
    pub glow: bool,
    /// Компактные вкладки.
    pub compact_tabs: bool,
    /// Фон новой вкладки: aurora, gradient, stars, waves, plain, image.
    pub wallpaper: String,
    /// Своя картинка для фона новой вкладки.
    pub wallpaper_img: String,
    /// Часы и приветствие на новой вкладке.
    pub clock: bool,
    /// Имя для приветствия.
    pub user_name: String,
    /// Разрешить расширения Chrome (распакованные, из папки extensions).
    pub extensions: bool,
}

impl Default for BrowserCfg {
    fn default() -> Self {
        BrowserCfg {
            restore: true,
            save_history: true,
            home: "https://www.google.com/".to_string(),
            search: "google".to_string(),
            doh: "off".to_string(),
            doh_custom: String::new(),
            proxy: String::new(),
            zoom: 100,
            block_popups: false,
            privacy_signals: true,
            clear_on_exit: false,
            ua: "default".to_string(),
            no_gpu: false,
            webrtc_safe: false,
            dl_dir: String::new(),
            lang: String::new(),
            dark_sites: true,
            theme: "petal".to_string(),
            accent_a: 0xEC4899,
            accent_b: 0x7C5CFF,
            ui_scale: 100,
            sidebar_w: 250,
            anim: true,
            glow: true,
            compact_tabs: false,
            wallpaper: "aurora".to_string(),
            wallpaper_img: String::new(),
            clock: true,
            user_name: String::new(),
            extensions: false,
        }
    }
}

pub const SEARCH_ENGINES: [(&str, &str, &str); 4] = [
    ("google", "Google", "https://www.google.com/search?q="),
    ("yandex", "Яндекс", "https://yandex.ru/search/?text="),
    ("ddg", "DuckDuckGo", "https://duckduckgo.com/?q="),
    ("bing", "Bing", "https://www.bing.com/search?q="),
];

pub const DOH_SERVERS: [(&str, &str, &str); 7] = [
    ("off", "Системный (как в Windows)", ""),
    ("cloudflare", "Cloudflare 1.1.1.1", "https://cloudflare-dns.com/dns-query"),
    ("google", "Google Public DNS", "https://dns.google/dns-query"),
    ("quad9", "Quad9", "https://dns.quad9.net/dns-query"),
    ("adguard", "AdGuard (без рекламы)", "https://dns.adguard-dns.com/dns-query"),
    ("yandex", "Яндекс DNS", "https://common.dns.yandex.net/dns-query"),
    ("comss", "Comss.one DNS", "https://dns.comss.one/dns-query"),
];

/// Готовые цветовые темы: (id, название, цвет A, цвет B).
pub const THEMES: [(&str, &str, u32, u32); 7] = [
    ("petal", "Лепесток", 0xEC4899, 0x7C5CFF),
    ("ocean", "Океан", 0x22D3EE, 0x3B82F6),
    ("forest", "Лес", 0x4ADE80, 0x06B6D4),
    ("sunset", "Закат", 0xFB923C, 0xEF4444),
    ("grape", "Виноград", 0xC084FC, 0x6366F1),
    ("gold", "Золото", 0xFACC15, 0xF97316),
    ("mono", "Графит", 0xE5E7EB, 0x9CA3AF),
];

pub const WALLPAPERS: [(&str, &str); 6] = [
    ("aurora", "Северное сияние"),
    ("gradient", "Градиент"),
    ("stars", "Звёздное небо"),
    ("waves", "Волны"),
    ("plain", "Однотонный"),
    ("image", "Своя картинка"),
];

/// Два акцентных цвета текущей темы.
pub fn theme_colors(c: &BrowserCfg) -> (u32, u32) {
    if c.theme == "custom" {
        return (c.accent_a & 0xFFFFFF, c.accent_b & 0xFFFFFF);
    }
    THEMES.iter().find(|t| t.0 == c.theme).map(|t| (t.2, t.3)).unwrap_or((THEMES[0].2, THEMES[0].3))
}

pub fn search_prefix(id: &str) -> &'static str {
    SEARCH_ENGINES.iter().find(|e| e.0 == id).map(|e| e.2).unwrap_or(SEARCH_ENGINES[0].2)
}

pub fn search_name(id: &str) -> &'static str {
    SEARCH_ENGINES.iter().find(|e| e.0 == id).map(|e| e.1).unwrap_or(SEARCH_ENGINES[0].1)
}

/// Адрес DoH-сервера для выбранного пункта, если защищённый DNS включён и адрес корректен.
pub fn doh_template(c: &BrowserCfg) -> Option<String> {
    if c.doh == "custom" {
        let t = c.doh_custom.trim();
        return (t.starts_with("https://") && !t.contains(char::is_whitespace)).then(|| t.to_string());
    }
    DOH_SERVERS.iter().find(|e| e.0 == c.doh && !e.2.is_empty()).map(|e| e.2.to_string())
}

fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Прокси в виде, пригодном для параметра Chromium; пустая строка, если он не задан или записан неверно.
pub fn proxy_arg(c: &BrowserCfg) -> String {
    let p = c.proxy.trim();
    let ok = (p.starts_with("http://") || p.starts_with("socks5://")) && p.len() > 9 && !p.contains(char::is_whitespace) && !p.contains('"');
    if ok { format!("--proxy-server={p}") } else { String::new() }
}

/// Параметры запуска встроенного браузера. Начало повторяет то, что wry добавляет сам, иначе оно пропадёт.
pub fn browser_args(c: &BrowserCfg) -> String {
    let mut a = String::from("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required");
    if let Some(t) = doh_template(c) {
        a.push_str(&format!(
            " --enable-features=DnsOverHttps<DoHTrial --force-fieldtrials=DoHTrial/Group1 --force-fieldtrial-params=DoHTrial.Group1:Fallback/true/Templates/{}",
            pct(&t)
        ));
    }
    let proxy = proxy_arg(c);
    if !proxy.is_empty() {
        a.push(' ');
        a.push_str(&proxy);
    }
    if c.no_gpu {
        a.push_str(" --disable-gpu");
    }
    if c.webrtc_safe {
        a.push_str(" --force-webrtc-ip-handling-policy=disable_non_proxied_udp");
    }
    match c.lang.as_str() {
        "ru" => a.push_str(" --accept-lang=ru-RU,ru"),
        "en" => a.push_str(" --accept-lang=en-US,en"),
        _ => {}
    }
    a
}

/// Строка «User-Agent» для выбранного пункта (пусто = как у самого браузера).
pub fn user_agent(id: &str) -> &'static str {
    match id {
        "chrome" => "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
        "firefox" => "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:132.0) Gecko/20100101 Firefox/132.0",
        "mobile" => "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36",
        _ => "",
    }
}

pub fn cfg_to_text(c: &BrowserCfg) -> String {
    let one = |b: bool| b as u8;
    format!(
        "restore={}\nsave_history={}\nhome={}\nsearch={}\ndoh={}\ndoh_custom={}\nproxy={}\nzoom={}\nblock_popups={}\nprivacy_signals={}\nclear_on_exit={}\nua={}\nno_gpu={}\nwebrtc_safe={}\ndl_dir={}\nlang={}\ndark_sites={}\ntheme={}\naccent_a={:06x}\naccent_b={:06x}\nui_scale={}\nsidebar_w={}\nanim={}\nglow={}\ncompact_tabs={}\nwallpaper={}\nwallpaper_img={}\nclock={}\nuser_name={}\nextensions={}\n",
        one(c.restore),
        one(c.save_history),
        c.home,
        c.search,
        c.doh,
        c.doh_custom,
        c.proxy,
        c.zoom,
        one(c.block_popups),
        one(c.privacy_signals),
        one(c.clear_on_exit),
        c.ua,
        one(c.no_gpu),
        one(c.webrtc_safe),
        c.dl_dir,
        c.lang,
        one(c.dark_sites),
        c.theme,
        c.accent_a & 0xFFFFFF,
        c.accent_b & 0xFFFFFF,
        c.ui_scale,
        c.sidebar_w,
        one(c.anim),
        one(c.glow),
        one(c.compact_tabs),
        c.wallpaper,
        c.wallpaper_img,
        one(c.clock),
        c.user_name,
        one(c.extensions),
    )
}

/// Идентификатор расширения (32 буквы a–p) из ссылки Chrome Web Store или из самого идентификатора.
pub fn ext_id_from(s: &str) -> Option<String> {
    let clean = |t: &str| t.len() == 32 && t.bytes().all(|b| (b'a'..=b'p').contains(&b));
    let path = s.trim().split(['?', '#']).next().unwrap_or("");
    path.split('/').rev().find(|seg| clean(seg)).map(|t| t.to_string())
}

/// Где в файле .crx начинается обычный zip-архив (форматы CRX2 и CRX3).
pub fn crx_zip_offset(d: &[u8]) -> Option<usize> {
    if d.len() < 16 || &d[0..4] != b"Cr24" {
        return None;
    }
    let u = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
    let off = match u(4) {
        3 => 12usize.checked_add(u(8))?,
        2 => 16usize.checked_add(u(8))?.checked_add(u(12))?,
        _ => return None,
    };
    (off < d.len()).then_some(off)
}

pub fn cfg_from_text(s: &str) -> BrowserCfg {
    let mut c = BrowserCfg::default();
    for line in s.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let on = v == "1";
        match k {
            "restore" => c.restore = on,
            "save_history" => c.save_history = on,
            "home" if !v.is_empty() => c.home = v.to_string(),
            "search" if SEARCH_ENGINES.iter().any(|e| e.0 == v) => c.search = v.to_string(),
            "doh" if v == "custom" || DOH_SERVERS.iter().any(|e| e.0 == v) => c.doh = v.to_string(),
            "doh_custom" => c.doh_custom = v.to_string(),
            "proxy" => c.proxy = v.to_string(),
            "zoom" => c.zoom = v.parse::<u32>().unwrap_or(100).clamp(50, 200),
            "block_popups" => c.block_popups = on,
            "privacy_signals" => c.privacy_signals = on,
            "clear_on_exit" => c.clear_on_exit = on,
            "ua" if matches!(v, "default" | "chrome" | "firefox" | "mobile") => c.ua = v.to_string(),
            "no_gpu" => c.no_gpu = on,
            "webrtc_safe" => c.webrtc_safe = on,
            "dl_dir" => c.dl_dir = v.to_string(),
            "lang" if matches!(v, "" | "ru" | "en") => c.lang = v.to_string(),
            "dark_sites" => c.dark_sites = on,
            "theme" if v == "custom" || THEMES.iter().any(|t| t.0 == v) => c.theme = v.to_string(),
            "accent_a" => c.accent_a = u32::from_str_radix(v, 16).unwrap_or(c.accent_a) & 0xFFFFFF,
            "accent_b" => c.accent_b = u32::from_str_radix(v, 16).unwrap_or(c.accent_b) & 0xFFFFFF,
            "ui_scale" => c.ui_scale = v.parse::<u32>().unwrap_or(100).clamp(80, 140),
            "sidebar_w" => c.sidebar_w = v.parse::<u32>().unwrap_or(250).clamp(240, 340),
            "anim" => c.anim = on,
            "glow" => c.glow = on,
            "compact_tabs" => c.compact_tabs = on,
            "wallpaper" if WALLPAPERS.iter().any(|w| w.0 == v) => c.wallpaper = v.to_string(),
            "wallpaper_img" => c.wallpaper_img = v.to_string(),
            "clock" => c.clock = on,
            "user_name" => c.user_name = v.chars().take(30).collect(),
            "extensions" => c.extensions = on,
            _ => {}
        }
    }
    c
}

pub fn load_cfg() -> BrowserCfg {
    std::fs::read_to_string(BROWSER_FILE).map(|s| cfg_from_text(&s)).unwrap_or_default()
}

pub fn save_cfg(c: &BrowserCfg) {
    let _ = std::fs::write(BROWSER_FILE, cfg_to_text(c));
}

// ───────────── Прошлая сессия ─────────────

pub fn session_to_text(urls: &[String], active: usize) -> String {
    let mut s = format!("active={active}\n");
    for u in urls {
        s.push_str(&esc(u));
        s.push('\n');
    }
    s
}

/// Адреса вкладок и номер активной. Пустой список значит «восстанавливать нечего».
pub fn session_from_text(s: &str) -> (Vec<String>, usize) {
    let mut lines = s.lines();
    let mut active = 0usize;
    let mut urls = Vec::new();
    if let Some(first) = lines.next() {
        match first.strip_prefix("active=") {
            Some(n) => active = n.trim().parse().unwrap_or(0),
            None => urls.push(unesc(first)),
        }
    }
    for l in lines {
        let u = unesc(l);
        if !u.trim().is_empty() {
            urls.push(u);
        }
    }
    urls.retain(|u| !u.trim().is_empty());
    urls.truncate(MAX_TABS);
    if urls.is_empty() {
        return (urls, 0);
    }
    let active = active.min(urls.len() - 1);
    (urls, active)
}

pub fn load_session() -> (Vec<String>, usize) {
    std::fs::read_to_string(SESSION_FILE).map(|s| session_from_text(&s)).unwrap_or_default()
}

// ───────────── История ─────────────

#[derive(Clone, PartialEq, Debug)]
pub struct Visit {
    /// Секунды с 1970 года.
    pub at: u64,
    pub url: String,
    pub title: String,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn history_to_text(v: &[Visit]) -> String {
    let mut s = String::new();
    for e in v {
        s.push_str(&format!("{}\t{}\t{}\n", e.at, esc(&e.url), esc(&e.title)));
    }
    s
}

pub fn history_from_text(s: &str) -> Vec<Visit> {
    let mut v: Vec<Visit> = s
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            let at = p.next()?.parse().ok()?;
            let url = unesc(p.next()?);
            let title = unesc(p.next().unwrap_or(""));
            (!url.is_empty()).then_some(Visit { at, url, title })
        })
        .collect();
    trim_history(&mut v);
    v
}

/// Оставляет только самые свежие записи (в списке старые идут первыми).
pub fn trim_history(v: &mut Vec<Visit>) {
    if v.len() > MAX_HISTORY {
        let extra = v.len() - MAX_HISTORY;
        v.drain(..extra);
    }
}

pub fn load_history() -> Vec<Visit> {
    std::fs::read_to_string(HISTORY_FILE).map(|s| history_from_text(&s)).unwrap_or_default()
}

pub fn save_history(v: &[Visit]) {
    let _ = std::fs::write(HISTORY_FILE, history_to_text(v));
}

/// Только обычные сайты попадают в историю.
pub fn is_loggable(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// «5 мин назад», «вчера» и т. п.
pub fn ago(now: u64, at: u64) -> String {
    let d = now.saturating_sub(at);
    match d {
        0..=59 => "только что".to_string(),
        60..=3599 => format!("{} мин назад", d / 60),
        3600..=86_399 => format!("{} ч назад", d / 3600),
        86_400..=172_799 => "вчера".to_string(),
        _ => format!("{} дн назад", d / 86_400),
    }
}

// ───────────── Загрузки ─────────────

pub const DL_ACTIVE: u8 = 0;
pub const DL_DONE: u8 = 1;
pub const DL_FAILED: u8 = 2;
const MAX_DOWNLOADS: usize = 200;

#[derive(Clone, PartialEq, Debug)]
pub struct Download {
    pub at: u64,
    pub state: u8,
    pub path: String,
    pub url: String,
}

impl Download {
    /// Имя файла без папки.
    pub fn name(&self) -> String {
        self.path.rsplit(['\\', '/']).next().unwrap_or(&self.path).to_string()
    }
}

pub fn downloads_to_text(v: &[Download]) -> String {
    let mut s = String::new();
    for d in v {
        s.push_str(&format!("{}\t{}\t{}\t{}\n", d.at, d.state, esc(&d.path), esc(&d.url)));
    }
    s
}

/// Загрузка, которая не успела закончиться до закрытия программы, считается прерванной.
pub fn downloads_from_text(s: &str) -> Vec<Download> {
    let mut v: Vec<Download> = s
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(4, '\t');
            let at = p.next()?.parse().ok()?;
            let state: u8 = p.next()?.parse().ok()?;
            let path = unesc(p.next()?);
            let url = unesc(p.next().unwrap_or(""));
            (!path.is_empty()).then_some(Download { at, state: if state == DL_ACTIVE { DL_FAILED } else { state }, path, url })
        })
        .collect();
    if v.len() > MAX_DOWNLOADS {
        let extra = v.len() - MAX_DOWNLOADS;
        v.drain(..extra);
    }
    v
}

pub fn load_downloads() -> Vec<Download> {
    std::fs::read_to_string(DOWNLOADS_FILE).map(|s| downloads_from_text(&s)).unwrap_or_default()
}

pub fn save_downloads(v: &[Download]) {
    let _ = std::fs::write(DOWNLOADS_FILE, downloads_to_text(v));
}

/// Программы и сценарии из интернета не запускаем одним щелчком: их можно только показать в папке.
pub fn is_risky_file(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    ["exe", "msi", "bat", "cmd", "com", "scr", "ps1", "vbs", "vbe", "js", "jse", "wsf", "hta", "lnk", "reg", "jar", "msix", "appx"]
        .contains(&ext.as_str())
}

// ───────────── Пароли ─────────────

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Entry {
    pub site: String,
    pub login: String,
    pub password: String,
}

#[derive(Debug, PartialEq)]
pub enum VaultError {
    WrongPassword,
    Corrupt,
}

const MAGIC: &[u8; 4] = b"PTL1";

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // Без случайных чисел шифровать нельзя: лучше остановиться, чем взять предсказуемые.
    getrandom::getrandom(&mut b).expect("нет источника случайных чисел");
    b
}

pub fn new_salt() -> [u8; 16] {
    random_bytes::<16>()
}

pub fn derive_key(password: &str, salt: &[u8; 16]) -> [u8; 32] {
    let params = Params::new(19_456, 2, 1, Some(32)).expect("параметры Argon2");
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .expect("Argon2");
    key
}

fn entries_to_text(v: &[Entry]) -> String {
    let mut s = String::new();
    for e in v {
        s.push_str(&format!("{}\t{}\t{}\n", esc(&e.site), esc(&e.login), esc(&e.password)));
    }
    s
}

fn entries_from_text(s: &str) -> Vec<Entry> {
    s.lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            Some(Entry { site: unesc(p.next()?), login: unesc(p.next()?), password: unesc(p.next()?) })
        })
        .collect()
}

/// Шифрует записи ключом; для каждой записи на диск берётся новое одноразовое число.
pub fn vault_encrypt(entries: &[Entry], key: &[u8; 32], salt: &[u8; 16]) -> Vec<u8> {
    let nonce = random_bytes::<12>();
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), entries_to_text(entries).as_bytes())
        .expect("шифрование");
    let mut out = Vec::with_capacity(4 + 16 + 12 + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

pub fn vault_salt(bytes: &[u8]) -> Result<[u8; 16], VaultError> {
    if bytes.len() < 4 + 16 + 12 + 16 || &bytes[..4] != MAGIC {
        return Err(VaultError::Corrupt);
    }
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&bytes[4..20]);
    Ok(salt)
}

/// Расшифровывает записи уже готовым ключом (`WrongPassword`, если ключ не подошёл).
pub fn vault_decrypt(bytes: &[u8], key: &[u8; 32]) -> Result<Vec<Entry>, VaultError> {
    vault_salt(bytes)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let plain = cipher.decrypt(Nonce::from_slice(&bytes[20..32]), &bytes[32..]).map_err(|_| VaultError::WrongPassword)?;
    let text = String::from_utf8(plain).map_err(|_| VaultError::Corrupt)?;
    Ok(entries_from_text(&text))
}

const LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
const DIGITS: &[u8] = b"23456789";
const SYMBOLS: &[u8] = b"!@#$%^&*-_=+?";

/// Случайный пароль: есть строчные, прописные, цифры и знаки, без похожих символов (0/O, 1/l).
pub fn generate_password(len: usize) -> String {
    let len = len.clamp(8, 64);
    let all: Vec<u8> = [LOWER, UPPER, DIGITS, SYMBOLS].concat();
    let pick = |set: &[u8]| -> u8 {
        // Отбрасываем «хвост», чтобы не было перекоса из-за деления с остатком.
        let limit = 256 - (256 % set.len());
        loop {
            let b = random_bytes::<1>()[0] as usize;
            if b < limit {
                return set[b % set.len()];
            }
        }
    };
    let mut chars: Vec<u8> = vec![pick(LOWER), pick(UPPER), pick(DIGITS), pick(SYMBOLS)];
    while chars.len() < len {
        chars.push(pick(&all));
    }
    // Перемешивание Фишера: обязательные символы не должны стоять в начале.
    for i in (1..chars.len()).rev() {
        let limit = 256 - (256 % (i + 1));
        let j = loop {
            let b = random_bytes::<1>()[0] as usize;
            if b < limit {
                break b % (i + 1);
            }
        };
        chars.swap(i, j);
    }
    String::from_utf8(chars).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_roundtrip_and_interrupted() {
        let v = vec![
            Download { at: 5, state: DL_DONE, path: "C:\\Users\\a\\Downloads\\файл\tодин.zip".into(), url: "https://x.ru/a.zip".into() },
            Download { at: 9, state: DL_ACTIVE, path: "C:\\Downloads\\b.pdf".into(), url: String::new() },
        ];
        let back = downloads_from_text(&downloads_to_text(&v));
        assert_eq!(back[0], v[0]);
        assert_eq!(back[1].state, DL_FAILED); // не доехала до конца
        assert_eq!(back[0].name(), "файл\tодин.zip");
        assert_eq!(Download { at: 0, state: 1, path: "/home/a/b.txt".into(), url: String::new() }.name(), "b.txt");
    }

    #[test]
    fn risky_files() {
        assert!(is_risky_file("C:\\Downloads\\setup.EXE"));
        assert!(is_risky_file("a.ps1"));
        assert!(!is_risky_file("photo.png"));
        assert!(!is_risky_file("report.pdf"));
        assert!(!is_risky_file("noext"));
    }

    #[test]
    fn data_dir_choice() {
        use std::ffi::OsString;
        use std::path::PathBuf;
        let appdata = Some(OsString::from("C:\\Users\\a\\AppData\\Local"));
        let exe = Some(PathBuf::from("D:\\Petal"));
        assert_eq!(pick_data_dir(appdata.clone(), exe.clone(), false), Some(PathBuf::from("C:\\Users\\a\\AppData\\Local").join("Petal")));
        assert_eq!(pick_data_dir(appdata, exe.clone(), true), exe);
        assert_eq!(pick_data_dir(None, exe.clone(), false), None);
        assert_eq!(pick_data_dir(Some(OsString::new()), exe, false), None);
    }

    #[test]
    fn cfg_roundtrip_and_defaults() {
        let c = BrowserCfg {
            restore: false,
            home: "https://ya.ru/".into(),
            search: "ddg".into(),
            doh: "quad9".into(),
            proxy: "socks5://127.0.0.1:1080".into(),
            zoom: 125,
            block_popups: true,
            clear_on_exit: true,
            ua: "firefox".into(),
            dl_dir: "D:\\Загрузки".into(),
            lang: "en".into(),
            dark_sites: false,
            theme: "custom".into(),
            accent_a: 0x112233,
            accent_b: 0xAABBCC,
            ui_scale: 120,
            sidebar_w: 300,
            anim: false,
            glow: false,
            compact_tabs: true,
            wallpaper: "stars".into(),
            user_name: "Аня".into(),
            ..BrowserCfg::default()
        };
        assert_eq!(cfg_from_text(&cfg_to_text(&c)), c);
        assert_eq!(cfg_from_text(""), BrowserCfg::default());
        assert!(cfg_from_text("мусор\nrestore=1").restore);
    }

    #[test]
    fn browser_args_follow_settings() {
        let mut c = BrowserCfg::default();
        let base = browser_args(&c);
        assert!(base.contains("msSmartScreenProtection") && base.contains("autoplay-policy"));
        assert!(!base.contains("DnsOverHttps") && !base.contains("proxy-server"));
        c.doh = "cloudflare".into();
        assert!(browser_args(&c).contains("Templates/https%3A%2F%2Fcloudflare-dns.com%2Fdns-query"));
        c.doh = "custom".into();
        c.doh_custom = "http://плохо".into();
        assert!(!browser_args(&c).contains("DnsOverHttps"));
        c.doh_custom = "https://dns.example/dns-query".into();
        assert!(browser_args(&c).contains("dns.example"));
        c.proxy = "socks5://127.0.0.1:1080".into();
        c.no_gpu = true;
        c.lang = "ru".into();
        let a = browser_args(&c);
        assert!(a.contains("--proxy-server=socks5://127.0.0.1:1080") && a.contains("--disable-gpu") && a.contains("ru-RU"));
        c.proxy = "ftp://x".into();
        assert!(!browser_args(&c).contains("proxy-server"));
    }

    #[test]
    fn extension_helpers() {
        let id = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
        assert_eq!(ext_id_from(id).as_deref(), Some(id));
        assert_eq!(ext_id_from(&format!("https://chromewebstore.google.com/detail/ublock-origin/{id}?hl=ru")).as_deref(), Some(id));
        assert_eq!(ext_id_from("https://example.com/abc"), None);
        assert_eq!(ext_id_from("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"), None);
        let mut crx3 = b"Cr24".to_vec();
        crx3.extend_from_slice(&3u32.to_le_bytes());
        crx3.extend_from_slice(&5u32.to_le_bytes());
        crx3.extend_from_slice(&[0; 5]);
        crx3.extend_from_slice(b"PK\x03\x04");
        assert_eq!(crx_zip_offset(&crx3), Some(17));
        assert_eq!(crx_zip_offset(b"not a crx at all.."), None);
        assert_eq!(crx_zip_offset(b"Cr24\x09\0\0\0xxxxxxxxxxxx"), None);
    }

    #[test]
    fn cfg_ignores_bad_values() {
        let c = cfg_from_text("search=нет\nzoom=900\nua=бред\ndoh=x\nlang=de\ntheme=bad\nui_scale=5\nsidebar_w=9999\nwallpaper=zzz");
        assert_eq!(c, BrowserCfg { zoom: 200, ui_scale: 80, sidebar_w: 340, ..BrowserCfg::default() });
        assert_eq!(theme_colors(&BrowserCfg { theme: "ocean".into(), ..BrowserCfg::default() }), (0x22D3EE, 0x3B82F6));
        assert_eq!(search_prefix("bing"), "https://www.bing.com/search?q=");
    }

    #[test]
    fn session_roundtrip_and_limits() {
        let urls = vec!["https://a.ru/".to_string(), "https://b.ru/?x=1&y=2".to_string()];
        assert_eq!(session_from_text(&session_to_text(&urls, 1)), (urls.clone(), 1));
        // номер активной вкладки за пределами списка
        assert_eq!(session_from_text(&session_to_text(&urls, 9)).1, 1);
        assert_eq!(session_from_text(""), (vec![], 0));
        let many: Vec<String> = (0..100).map(|i| format!("https://s{i}.ru/")).collect();
        assert_eq!(session_from_text(&session_to_text(&many, 0)).0.len(), MAX_TABS);
    }

    #[test]
    fn history_roundtrip_with_tabs_in_title() {
        let v = vec![
            Visit { at: 10, url: "https://a.ru/".into(), title: "Привет\tмир\nстрока".into() },
            Visit { at: 20, url: "https://b.ru/".into(), title: String::new() },
        ];
        assert_eq!(history_from_text(&history_to_text(&v)), v);
        assert!(history_from_text("не число\thttps://x\tt\n").is_empty());
    }

    #[test]
    fn history_is_trimmed_to_newest() {
        let mut v: Vec<Visit> = (0..MAX_HISTORY as u64 + 5).map(|i| Visit { at: i, url: format!("https://x{i}/"), title: String::new() }).collect();
        trim_history(&mut v);
        assert_eq!(v.len(), MAX_HISTORY);
        assert_eq!(v[0].at, 5);
    }

    #[test]
    fn only_websites_are_logged() {
        assert!(is_loggable("https://ya.ru/"));
        assert!(!is_loggable("about:blank"));
        assert!(!is_loggable("file:///C:/a.txt"));
    }

    #[test]
    fn relative_time() {
        assert_eq!(ago(100, 100), "только что");
        assert_eq!(ago(1000, 700), "5 мин назад");
        assert_eq!(ago(20_000, 20_000 - 7300), "2 ч назад");
        assert_eq!(ago(200_000, 200_000 - 90_000), "вчера");
        assert_eq!(ago(1_000_000, 1_000_000 - 400_000), "4 дн назад");
    }

    #[test]
    fn vault_roundtrip_and_wrong_password() {
        let salt = new_salt();
        let key = derive_key("мой пароль", &salt);
        let e = vec![Entry { site: "ya.ru".into(), login: "user\tname".into(), password: "p@ss\nw\\rd".into() }];
        let bytes = vault_encrypt(&e, &key, &salt);
        // пароль не лежит открытым текстом
        assert!(!String::from_utf8_lossy(&bytes).contains("ya.ru"));
        let salt2 = vault_salt(&bytes).unwrap();
        assert_eq!(salt2, salt);
        assert_eq!(vault_decrypt(&bytes, &derive_key("мой пароль", &salt2)).unwrap(), e);
        assert_eq!(vault_decrypt(&bytes, &derive_key("не тот", &salt2)), Err(VaultError::WrongPassword));
    }

    #[test]
    fn vault_detects_corruption() {
        let salt = new_salt();
        let key = derive_key("x", &salt);
        let mut bytes = vault_encrypt(&[Entry::default()], &key, &salt);
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(vault_decrypt(&bytes, &key).is_err());
        assert_eq!(vault_salt(b"short"), Err(VaultError::Corrupt));
        assert_eq!(vault_salt(&[0u8; 64]), Err(VaultError::Corrupt));
    }

    #[test]
    fn same_entries_encrypt_differently() {
        let salt = new_salt();
        let key = derive_key("x", &salt);
        let e = vec![Entry::default()];
        assert_ne!(vault_encrypt(&e, &key, &salt), vault_encrypt(&e, &key, &salt));
    }

    #[test]
    fn generated_passwords_are_varied_and_complete() {
        let p = generate_password(16);
        assert_eq!(p.chars().count(), 16);
        assert!(p.chars().any(|c| c.is_ascii_lowercase()));
        assert!(p.chars().any(|c| c.is_ascii_uppercase()));
        assert!(p.chars().any(|c| c.is_ascii_digit()));
        assert!(p.chars().any(|c| !c.is_ascii_alphanumeric()));
        assert_ne!(generate_password(16), generate_password(16));
        assert_eq!(generate_password(2).len(), 8);
    }
}
