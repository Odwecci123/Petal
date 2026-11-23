#![cfg_attr(not(windows), allow(dead_code))]
#![allow(clippy::too_many_arguments)]
// Petal: браузер с вкладками и набор функций (саундпад, калькулятор, случайные числа, колесо удачи) в тёмном стиле.
// Запуск: cargo run --release

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Key, Margin, RichText,
    ScrollArea, Sense, Stroke, Vec2,
};
use rodio::Source;
mod store;
use std::collections::HashMap;
use std::f64::consts::TAU;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ───────────────────────── Цвета ─────────────────────────

const BG: Color32 = Color32::from_rgb(19, 19, 26);
const SIDE_BG: Color32 = Color32::from_rgb(17, 17, 23);
const FIELD_BG: Color32 = Color32::from_rgb(34, 34, 46);
const CARD_BG: Color32 = Color32::from_rgb(27, 27, 37);
static ACC_A: AtomicU32 = AtomicU32::new(0xEC4899);
static ACC_B: AtomicU32 = AtomicU32::new(0x7C5CFF);
static ANIM_ON: AtomicBool = AtomicBool::new(true);

/// Случайное число от 0 до n (не включая n).
fn rand_below(n: u32) -> u32 {
    let mut b = [0u8; 4];
    if getrandom::getrandom(&mut b).is_err() {
        return 1;
    }
    u32::from_le_bytes(b) % n.max(1)
}

fn rgb(n: u32) -> Color32 {
    Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
}
fn accent_a() -> Color32 {
    rgb(ACC_A.load(Ordering::Relaxed))
}
fn accent_b() -> Color32 {
    rgb(ACC_B.load(Ordering::Relaxed))
}
/// Тот же цвет, но светлее: для состояния «наведено».
fn lit(c: Color32) -> Color32 {
    mix(c, Color32::WHITE, 0.24)
}
fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}
/// Светлый оттенок второго акцента: для подсветки текущего пункта и ссылок.
fn link() -> Color32 {
    mix(accent_b(), Color32::WHITE, 0.4)
}
/// Анимации egui, которые можно выключить в настройках.
trait AnimExt {
    fn abt(&self, id: egui::Id, v: bool, t: f32) -> f32;
    fn avt(&self, id: egui::Id, v: f32, t: f32) -> f32;
}
impl AnimExt for egui::Context {
    fn abt(&self, id: egui::Id, v: bool, t: f32) -> f32 {
        self.animate_bool_with_time(id, v, if ANIM_ON.load(Ordering::Relaxed) { t } else { 0.0 })
    }
    fn avt(&self, id: egui::Id, v: f32, t: f32) -> f32 {
        self.animate_value_with_time(id, v, if ANIM_ON.load(Ordering::Relaxed) { t } else { 0.0 })
    }
}
const TEXT: Color32 = Color32::from_rgb(232, 234, 237);
const MUTED: Color32 = Color32::from_rgb(154, 160, 166);
const LINK: Color32 = Color32::from_rgb(160, 168, 255);
const RED: Color32 = Color32::from_rgb(242, 139, 130);
const YELLOW: Color32 = Color32::from_rgb(253, 214, 99);
const GREEN: Color32 = Color32::from_rgb(129, 201, 149);
const PURPLE: Color32 = Color32::from_rgb(197, 138, 249);
const ORANGE: Color32 = Color32::from_rgb(255, 167, 38);


// ───────────────────────── Разделы панели функций ─────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Sounds,
    Calc,
    Random,
    Wheel,
    History,
    Passwords,
    Settings,
    Downloads,
}

/// Эти окна открываются из меню с тремя точками. «Загрузки» открываются кнопкой внизу колонки вкладок,
/// остальные окна из меню со стикером.
fn in_dots(t: Tool) -> bool {
    matches!(t, Tool::History | Tool::Passwords | Tool::Settings)
}

const TOOLS: [(Tool, &str, Color32); 8] = [
    (Tool::Sounds, "Саундпад", ORANGE),
    (Tool::Calc, "Калькулятор", LINK),
    (Tool::Random, "Случайные числа", YELLOW),
    (Tool::Wheel, "Колесо удачи", GREEN),
    (Tool::History, "История", LINK),
    (Tool::Passwords, "Пароли", YELLOW),
    (Tool::Settings, "Настройки", MUTED),
    (Tool::Downloads, "Загрузки", GREEN),
];

// ───────────────────────── Расширения Chrome ─────────────────────────

fn extensions_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_default().join("extensions")
}

/// Имя расширения из его manifest.json (с учётом переводов «__MSG_…__»).
fn extension_name(dir: &Path) -> String {
    let fallback = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else { return fallback };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else { return fallback };
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
    if let Some(key) = name.strip_prefix("__MSG_").and_then(|k| k.strip_suffix("__")) {
        for lang in ["ru", "en", "en_US"] {
            let Ok(m) = std::fs::read_to_string(dir.join("_locales").join(lang).join("messages.json")) else { continue };
            let Ok(mv) = serde_json::from_str::<serde_json::Value>(m.trim_start_matches('\u{feff}')) else { continue };
            let found = mv.as_object().and_then(|o| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(key))).and_then(|(_, e)| e.get("message")).and_then(|m| m.as_str());
            if let Some(f) = found {
                return f.to_string();
            }
        }
        return fallback;
    }
    if name.is_empty() { fallback } else { name }
}

/// Установленные расширения: (папка, название).
fn list_extensions() -> Vec<(String, String)> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(extensions_dir()) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                v.push((e.file_name().to_string_lossy().to_string(), extension_name(&e.path())));
            }
        }
    }
    v.sort();
    v
}

/// Версия программы: из тега релиза (его передаёт сборка), а без него из Cargo.toml.
const APP_VERSION: &str = match option_env!("PETAL_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

/// Спрашивает у GitHub последний релиз. `None`, если он не новее установленной версии.
fn check_update() -> Result<Option<store::UpdateInfo>, String> {
    let mut req = ehttp::Request::get("https://api.github.com/repos/Odwecci123/petal/releases/latest");
    req.headers.insert("User-Agent", "Petal-updater");
    req.headers.insert("Accept", "application/vnd.github+json");
    let resp = ehttp::fetch_blocking(&req).map_err(|e| format!("Нет связи с GitHub: {e}"))?;
    if resp.status != 200 {
        return Err(format!("GitHub ответил кодом {}.", resp.status));
    }
    let text = String::from_utf8_lossy(&resp.bytes).to_string();
    let info = store::parse_release(&text).ok_or("Не удалось разобрать ответ GitHub.")?;
    Ok(store::is_newer(&info.version, APP_VERSION).then_some(info))
}

/// Скачивает установщик во временную папку и возвращает путь к нему.
fn download_installer(info: &store::UpdateInfo) -> Result<std::path::PathBuf, String> {
    if !info.installer.starts_with(store::RELEASE_PREFIX) {
        return Err("У релиза нет установщика.".to_string());
    }
    let resp = ehttp::fetch_blocking(&ehttp::Request::get(&info.installer)).map_err(|e| format!("Не удалось скачать: {e}"))?;
    if resp.status != 200 || resp.bytes.len() < 100_000 || !resp.bytes.starts_with(b"MZ") {
        return Err(format!("Скачивание не удалось (код {}).", resp.status));
    }
    let path = std::env::temp_dir().join(format!("Petal-Setup-{}.exe", info.version));
    std::fs::write(&path, &resp.bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Скачивает расширение из Chrome Web Store и распаковывает его в папку extensions. Возвращает его название.
fn install_extension(id: &str) -> Result<String, String> {
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=130.0.0.0&acceptformat=crx2,crx3&x=id%3D{id}%26installsource%3Dondemand%26uc"
    );
    let resp = ehttp::fetch_blocking(&ehttp::Request::get(url)).map_err(|e| format!("Не удалось скачать: {e}"))?;
    if resp.status != 200 {
        return Err(format!("Магазин ответил кодом {}. Проверьте ссылку или доступ к Google.", resp.status));
    }
    let off = store::crx_zip_offset(&resp.bytes).ok_or("Скачанный файл не похож на расширение.")?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&resp.bytes[off..])).map_err(|e| format!("Архив расширения повреждён: {e}"))?;
    let dir = extensions_dir().join(id);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else { continue };
        let out = dir.join(rel);
        if f.is_dir() {
            let _ = std::fs::create_dir_all(&out);
        } else {
            if let Some(parent) = out.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut file = File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, &mut file).map_err(|e| e.to_string())?;
        }
    }
    if !dir.join("manifest.json").exists() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("В архиве нет manifest.json: это не расширение.".to_string());
    }
    Ok(extension_name(&dir))
}

// ───────────────────────── Случайные числа ─────────────────────────

struct Rng {
    fallback: u64,
}

impl Rng {
    fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(88172645463325252);
        Rng { fallback: seed | 1 }
    }

    fn next(&mut self) -> u64 {
        let mut b = [0u8; 8];
        if getrandom::getrandom(&mut b).is_ok() {
            return u64::from_le_bytes(b);
        }
        let mut x = self.fallback;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.fallback = x;
        x
    }

    /// Число от 0 до n-1 без смещения.
    fn below(&mut self, n: u64) -> u64 {
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let x = self.next();
            if x < zone {
                return x % n;
            }
        }
    }

    /// Дробное число от 0 (включая) до 1 (не включая).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        lo + self.below((hi - lo) as u64 + 1) as i64
    }
}


// ───────────────────────── Вычисление выражений ─────────────────────────

struct Parser {
    s: Vec<char>,
    i: usize,
}

impl Parser {
    fn peek(&mut self) -> Option<char> {
        while self.i < self.s.len() && self.s[self.i].is_whitespace() {
            self.i += 1;
        }
        self.s.get(self.i).copied()
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some('+') => {
                    self.i += 1;
                    v += self.term()?;
                }
                Some('-') => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => break,
            }
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        loop {
            match self.peek() {
                Some('*') | Some('×') => {
                    self.i += 1;
                    v *= self.unary()?;
                }
                Some('/') | Some(':') => {
                    self.i += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return Err("Деление на ноль".into());
                    }
                    v /= d;
                }
                _ => break,
            }
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some('-') => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some('+') => {
                self.i += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.primary()?;
        if self.peek() == Some('^') {
            self.i += 1;
            let exp = self.unary()?;
            Ok(base.powf(exp))
        } else {
            Ok(base)
        }
    }

    fn primary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some('(') => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != Some(')') {
                    return Err("Не закрыта скобка".into());
                }
                self.i += 1;
                Ok(v)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == '.') {
                    self.i += 1;
                }
                let text: String = self.s[start..self.i].iter().collect();
                text.parse::<f64>().map_err(|_| format!("Неверное число: {text}"))
            }
            Some(c) if c.is_alphabetic() => {
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i].is_alphanumeric() {
                    self.i += 1;
                }
                let name: String = self.s[start..self.i].iter().collect::<String>().to_lowercase();
                match name.as_str() {
                    "pi" => return Ok(std::f64::consts::PI),
                    "e" => return Ok(std::f64::consts::E),
                    _ => {}
                }
                if self.peek() != Some('(') {
                    return Err(format!("Неизвестное имя: {name}"));
                }
                self.i += 1;
                let arg = self.expr()?;
                if self.peek() != Some(')') {
                    return Err("Не закрыта скобка".into());
                }
                self.i += 1;
                apply_fn(&name, arg)
            }
            Some(c) => Err(format!("Неожиданный символ: {c}")),
            None => Err("Выражение оборвано".into()),
        }
    }
}

fn apply_fn(name: &str, x: f64) -> Result<f64, String> {
    Ok(match name {
        "sqrt" | "корень" => x.sqrt(),
        "sin" => x.sin(),
        "cos" => x.cos(),
        "tan" => x.tan(),
        "ln" => x.ln(),
        "log" => x.log10(),
        "abs" => x.abs(),
        "exp" => x.exp(),
        "round" => x.round(),
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        _ => return Err(format!("Неизвестная функция: {name}")),
    })
}

fn eval(expr: &str) -> Result<f64, String> {
    let cleaned = expr.replace(',', ".");
    let mut p = Parser { s: cleaned.chars().collect(), i: 0 };
    let v = p.expr()?;
    if let Some(c) = p.peek() {
        return Err(format!("Неожиданный символ: {c}"));
    }
    if !v.is_finite() {
        return Err("Результат не определён".into());
    }
    Ok(v)
}

fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return "не число".into();
    }
    if v.abs() >= 1e15 {
        return format!("{v:e}");
    }
    let r = (v * 1e10).round() / 1e10;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

// ───────────────────────── Колесо удачи ─────────────────────────

const WHEEL_FILE: &str = "helper_wheel.txt";
const WHEEL_DEFAULT: &str = "Вариант 1\nВариант 2\nВариант 3\nВариант 4";
const WHEEL_PALETTE: [Color32; 6] = [
    Color32::from_rgb(244, 114, 182),
    Color32::from_rgb(167, 139, 250),
    Color32::from_rgb(96, 165, 250),
    Color32::from_rgb(52, 211, 153),
    Color32::from_rgb(251, 191, 36),
    Color32::from_rgb(251, 113, 133),
];

/// Варианты колеса: каждая непустая строка текста.
fn wheel_items(text: &str) -> Vec<String> {
    text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).map(String::from).collect()
}

/// Какой сектор оказался под стрелкой наверху. Сектор i занимает дугу
/// [angle + i*step, angle + (i+1)*step]: углы растут по часовой стрелке от направления «вправо»,
/// а стрелка смотрит вверх, то есть на -90°.
fn wheel_winner(angle: f64, n: usize) -> usize {
    let step = TAU / n as f64;
    let t = (-std::f64::consts::FRAC_PI_2 - angle).rem_euclid(TAU);
    ((t / step) as usize).min(n - 1)
}

fn sector_color(i: usize, n: usize) -> Color32 {
    let p = WHEEL_PALETTE.len();
    // Последний сектор не должен совпасть по цвету с первым.
    if n > 1 && i == n - 1 && i.is_multiple_of(p) {
        WHEEL_PALETTE[3]
    } else {
        WHEEL_PALETTE[i % p]
    }
}

/// Грань кубика с точками.
fn draw_die(ui: &mut egui::Ui, n: u8, size: f32, slot: f32) {
    let (slot_rect, _) = ui.allocate_exact_size(Vec2::new(slot, slot), Sense::hover());
    let rect = egui::Rect::from_center_size(slot_rect.center(), Vec2::splat(size));
    let p = ui.painter();
    p.rect_filled(rect.translate(Vec2::new(0.0, 5.0)), CornerRadius::same((size * 0.22) as u8), alpha(accent_a(), 60));
    grad_rrect(p, rect, size * 0.22, Color32::from_rgb(250, 250, 255), Color32::from_rgb(214, 208, 240), true);
    let d = size * 0.25;
    let c = rect.center();
    let spots: &[(f32, f32)] = match n {
        1 => &[(0.0, 0.0)],
        2 => &[(-1.0, -1.0), (1.0, 1.0)],
        3 => &[(-1.0, -1.0), (0.0, 0.0), (1.0, 1.0)],
        4 => &[(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)],
        5 => &[(-1.0, -1.0), (1.0, -1.0), (0.0, 0.0), (-1.0, 1.0), (1.0, 1.0)],
        _ => &[(-1.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (1.0, 1.0)],
    };
    for (x, y) in spots {
        p.circle_filled(c + Vec2::new(*x * d, *y * d), size * 0.085, Color32::from_rgb(70, 50, 140));
    }
}

/// Монетка: золотой круг с надписью.
fn draw_coin(ui: &mut egui::Ui, heads: bool, size: f32, slot: f32) {
    let (slot_rect, _) = ui.allocate_exact_size(Vec2::new(slot, slot), Sense::hover());
    let c = slot_rect.center();
    let p = ui.painter();
    let r = size / 2.0;
    p.circle_filled(c + Vec2::new(0.0, 5.0), r, Color32::from_rgba_unmultiplied(250, 204, 21, 50));
    p.circle_filled(c, r, Color32::from_rgb(202, 138, 4));
    p.circle_filled(c, r * 0.92, Color32::from_rgb(250, 204, 21));
    p.circle_stroke(c, r * 0.72, Stroke::new(2.0_f32, Color32::from_rgb(202, 138, 4)));
    p.text(c, Align2::CENTER_CENTER, if heads { "Орёл" } else { "Решка" }, FontId::proportional((size * 0.22).max(6.0)), Color32::from_rgb(120, 70, 0));
}

/// Рисует колесо со стрелкой. Возвращает true, если нажали на центр.
fn draw_wheel(ui: &mut egui::Ui, items: &[String], angle: f64, size: f32) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter_at(rect.expand(8.0));
    let c = rect.center();
    let r = size / 2.0 - 24.0;
    let n = items.len();
    let time = ui.input(|i| i.time);

    // Тень и ободок с лампочками.
    painter.circle_filled(c + Vec2::new(0.0, 6.0), r + 16.0, Color32::from_rgba_unmultiplied(0, 0, 0, 90));
    painter.circle_filled(c, r + 14.0, Color32::from_rgb(40, 34, 70));
    painter.circle_stroke(c, r + 14.0, Stroke::new(2.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 40)));
    let spinning_now = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("wheel_spinning"))).unwrap_or(false);
    for k in 0..24 {
        let a = TAU as f32 * k as f32 / 24.0;
        let on = if spinning_now { ((time * 8.0) as i64 + k as i64) % 2 == 0 } else { k % 2 == 0 };
        let col = if on { Color32::from_rgb(255, 220, 120) } else { Color32::from_rgb(120, 90, 160) };
        painter.circle_filled(c + Vec2::new(a.cos(), a.sin()) * (r + 7.0), 2.6, col);
    }

    if n == 0 {
        painter.circle(c, r, FIELD_BG, Stroke::new(2.0_f32, CARD_BG));
        painter.text(c, Align2::CENTER_CENTER, "Добавьте варианты", FontId::proportional(16.0), MUTED);
    } else if n == 1 {
        painter.circle_filled(c, r, sector_color(0, 1));
    } else {
        let step = TAU / n as f64;
        for i in 0..n {
            let a0 = angle + step * i as f64;
            let segs = ((step / 0.08).ceil() as usize).max(2);
            let mut pts = Vec::with_capacity(segs + 2);
            pts.push(c);
            for k in 0..=segs {
                let a = a0 + step * k as f64 / segs as f64;
                pts.push(c + Vec2::new(a.cos() as f32, a.sin() as f32) * r);
            }
            painter.add(egui::Shape::convex_polygon(pts, sector_color(i, n), Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 120))));
        }
    }

    // Подписи вдоль радиуса. Слева текст переворачиваем, чтобы он не шёл вверх ногами.
    let ink = Color32::from_rgb(28, 24, 44);
    if (1..=36).contains(&n) {
        let font = FontId::proportional(if n <= 8 { 17.0 } else if n <= 16 { 13.0 } else { 11.0 });
        let step = TAU / n as f64;
        for (i, text) in items.iter().enumerate() {
            let mid = angle + step * (i as f64 + 0.5);
            let (cos, sin) = (mid.cos() as f32, mid.sin() as f32);
            let dir = Vec2::new(cos, sin);
            let mut job = egui::text::LayoutJob::simple(text.clone(), font.clone(), ink, r * 0.62);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let galley = painter.layout_job(job);
            let (theta, start) = if cos >= 0.0 {
                (mid as f32, c + dir * (r * 0.28))
            } else {
                (mid as f32 + std::f32::consts::PI, c + dir * (r * 0.94))
            };
            let y_axis = Vec2::new(-theta.sin(), theta.cos());
            let pos = start - y_axis * (galley.size().y / 2.0);
            painter.add(egui::epaint::TextShape::new(pos, galley, ink).with_angle(theta));
        }
    }

    // Центр: кнопка «Крутить».
    let hub = egui::Rect::from_center_size(c, Vec2::splat(76.0));
    let resp = ui.interact(hub, ui.id().with("wheel_hub"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let t = hover_t(ui, &resp);
    painter.circle_filled(c, 38.0 + 3.0 * t, alpha(accent_a(), (50.0 + 60.0 * t) as u8));
    painter.circle_filled(c, 33.0 + 2.0 * t, mix(Color32::from_rgb(70, 44, 120), accent_b(), t));
    painter.circle_stroke(c, 33.0 + 2.0 * t, Stroke::new(2.5_f32, Color32::WHITE));
    painter.text(c, Align2::CENTER_CENTER, "Крутить", FontId::proportional(13.0), Color32::WHITE);

    // Стрелка сверху.
    let tip = egui::pos2(c.x, rect.top() + 36.0);
    painter.add(egui::Shape::convex_polygon(
        vec![egui::pos2(c.x - 12.0, rect.top() + 9.0), egui::pos2(c.x + 16.0, rect.top() + 9.0), tip + Vec2::new(2.0, 3.0)],
        Color32::from_rgba_unmultiplied(0, 0, 0, 90),
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![egui::pos2(c.x - 14.0, rect.top() + 6.0), egui::pos2(c.x + 14.0, rect.top() + 6.0), tip],
        accent_a(),
        Stroke::new(2.5_f32, Color32::WHITE),
    ));
    resp.clicked()
}

// ───────────────────────── Встроенный браузер ─────────────────────────

/// События загрузок из браузера: обработчики складывают их сюда, программа разбирает каждый кадр.
#[cfg_attr(not(windows), allow(dead_code))]
enum DlEvent {
    Started { url: String, path: PathBuf },
    Done { url: String, path: Option<PathBuf>, ok: bool },
}

static DL_EVENTS: Mutex<Vec<DlEvent>> = Mutex::new(Vec::new());
/// Настройки, которые браузер читает один раз при запуске (параметры движка, прокси, представление сайтам).
static FROZEN: std::sync::OnceLock<store::BrowserCfg> = std::sync::OnceLock::new();
/// Настройки, которые действуют сразу, без перезапуска.
static BLOCK_POPUPS: AtomicBool = AtomicBool::new(false);
static SEARCH_IDX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static DL_DIR: Mutex<String> = Mutex::new(String::new());
static ZOOM: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);

fn sync_live(c: &store::BrowserCfg) {
    BLOCK_POPUPS.store(c.block_popups, Ordering::Relaxed);
    ZOOM.store(c.zoom, Ordering::Relaxed);
    SEARCH_IDX.store(store::SEARCH_ENGINES.iter().position(|e| e.0 == c.search).unwrap_or(0), Ordering::Relaxed);
    if let Ok(mut d) = DL_DIR.lock() {
        if *d != c.dl_dir {
            *d = c.dl_dir.clone();
        }
    }
}

fn search_prefix_now() -> &'static str {
    store::SEARCH_ENGINES[SEARCH_IDX.load(Ordering::Relaxed).min(store::SEARCH_ENGINES.len() - 1)].2
}


fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Собственная стартовая страница Petal: лежит файлом в папке данных, логотип вшит прямо в страницу.
static NEWTAB_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static INCOGNITO_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

const NEWTAB_HTML: &str = r##"<!doctype html><html lang="ru"><head><meta charset="utf-8"><title>Новая вкладка</title>
<meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="dark">
<style>
html,body{margin:0;height:100%;background:#131316;color:#fff;font-family:"Segoe UI",system-ui,sans-serif;overflow:hidden}
body::before,body::after{content:"";position:fixed;width:46vmax;height:46vmax;border-radius:50%;filter:blur(90px);opacity:.28;z-index:0;pointer-events:none}
body::before{background:var(--a);left:-14vmax;top:-18vmax;animation:d1 22s ease-in-out infinite alternate}
body::after{background:var(--b);right:-16vmax;bottom:-22vmax;animation:d2 26s ease-in-out infinite alternate}
@keyframes d1{to{transform:translate(18vmax,10vmax) scale(1.15)}}
@keyframes d2{to{transform:translate(-16vmax,-8vmax) scale(1.2)}}
.w{position:relative;z-index:1;display:flex;flex-direction:column;align-items:center;padding-top:15vh}
.l{display:flex;align-items:center;gap:22px;margin-bottom:56px;animation:up .8s cubic-bezier(.2,.8,.2,1) both}
.l img{width:64px;height:64px;border-radius:16px;animation:fl 5s ease-in-out infinite;transition:transform .5s}
.l img:hover{transform:rotate(72deg) scale(1.1)}
.l span{font-size:68px;font-weight:700;letter-spacing:-1px;background:linear-gradient(90deg,#fff,color-mix(in srgb,var(--a) 55%,#fff),color-mix(in srgb,var(--b) 55%,#fff),#fff);background-size:300% 100%;-webkit-background-clip:text;background-clip:text;color:transparent;animation:sh 8s linear infinite}
@keyframes fl{50%{transform:translateY(-6px) rotate(-4deg)}}
@keyframes sh{to{background-position:-300% 0}}
@keyframes up{from{opacity:0;transform:translateY(22px)}to{opacity:1;transform:none}}
form{width:min(850px,86vw);position:relative;animation:up .8s .15s cubic-bezier(.2,.8,.2,1) both}
input{box-sizing:border-box;width:100%;height:54px;border:0;border-radius:27px;background:rgba(40,40,46,.85);color:#fff;font-size:17px;padding:0 24px 0 58px;outline:none;transition:box-shadow .25s,background .25s,transform .25s}
input::placeholder{color:#9a9aa3}
input:hover{background:rgba(52,52,60,.9)}
input:focus{background:rgba(52,52,60,.95);box-shadow:0 0 0 2px var(--b),0 8px 30px color-mix(in srgb,var(--b) 35%,transparent);transform:translateY(-1px)}
svg{position:absolute;left:21px;top:16px;transition:stroke .25s}
form:focus-within svg{stroke:color-mix(in srgb,var(--b) 50%,#fff)}
.tiles{position:relative;z-index:1;display:flex;flex-wrap:wrap;justify-content:center;gap:14px;width:min(850px,86vw);margin-top:44px;animation:up .8s .3s cubic-bezier(.2,.8,.2,1) both}
.t{position:relative;width:150px;height:104px;border-radius:22px;background:rgba(255,255,255,.06);border:1px solid rgba(255,255,255,.07);display:flex;flex-direction:column;align-items:center;justify-content:center;gap:10px;color:#e8e8f0;text-decoration:none;font-size:14px;cursor:pointer;transition:transform .2s,background .2s,box-shadow .2s;animation:pop .5s both}
.t:hover{transform:translateY(-4px);background:rgba(255,255,255,.11);box-shadow:0 12px 30px rgba(0,0,0,.35),0 0 0 1px color-mix(in srgb,var(--b) 55%,transparent)}
.t img,.t .ph{width:40px;height:40px;border-radius:11px;object-fit:cover}
.t .ph{display:flex;align-items:center;justify-content:center;font-size:20px;font-weight:700;color:#fff;background:linear-gradient(135deg,var(--a),var(--b))}
.t span{max-width:126px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.t .x{position:absolute;right:7px;top:7px;width:22px;height:22px;border-radius:50%;background:rgba(0,0,0,.45);color:#fff;font-size:14px;line-height:22px;text-align:center;opacity:0;transition:opacity .2s,background .2s}
.t:hover .x{opacity:1}.t .x:hover{background:var(--a)}
.t.add{border-style:dashed;color:#9a9aa3}
.t.add .pl{width:40px;height:40px;border-radius:11px;border:2px dashed rgba(255,255,255,.25);display:flex;align-items:center;justify-content:center;font-size:24px;color:color-mix(in srgb,var(--b) 50%,#fff);transition:transform .3s}
.t.add:hover .pl{transform:rotate(90deg)}
@keyframes pop{from{opacity:0;transform:scale(.85)}to{opacity:1;transform:none}}
#m{position:fixed;inset:0;z-index:5;background:rgba(10,10,14,.6);backdrop-filter:blur(6px);display:none;align-items:center;justify-content:center}
#m.on{display:flex;animation:fade .2s}
@keyframes fade{from{opacity:0}}
#m form{width:340px;background:#202029;border:1px solid rgba(255,255,255,.1);border-radius:22px;padding:22px;box-shadow:0 20px 60px rgba(0,0,0,.5);animation:pop .25s both}
#m h3{margin:0 0 14px;font-size:18px}
#m input{height:44px;border-radius:14px;padding:0 16px;margin-bottom:10px;font-size:15px;background:#2b2b36}
#m .r{display:flex;gap:10px;margin-top:6px}
#m button{flex:1;height:42px;border:0;border-radius:14px;font-size:15px;cursor:pointer;color:#fff;background:#2f2f3c;transition:filter .2s}
#m button:hover{filter:brightness(1.2)}
@property --ang{syntax:"<angle>";inherits:false;initial-value:0deg}
@keyframes spin{to{--ang:360deg}}
.bg{position:fixed;inset:0;z-index:0;display:none;overflow:hidden}
.bg canvas{position:absolute;inset:0;width:100%;height:100%;display:block}
body.wp-veil .bg,body.wp-ether .bg,body.wp-tunnel .bg{display:block}
#f{isolation:isolate}
#f::before,#f::after,.t::before{content:"";position:absolute;pointer-events:none;background:conic-gradient(from var(--ang),transparent 0 48%,var(--a) 60%,#fff 71%,var(--b) 80%,transparent 94%);animation:spin 5s linear infinite}
#f::before,.t::before{inset:-1.5px;padding:1.5px;border-radius:29px;opacity:0;transition:opacity .35s;-webkit-mask:linear-gradient(#000 0 0) content-box,linear-gradient(#000 0 0);-webkit-mask-composite:xor;mask:linear-gradient(#000 0 0) content-box exclude,linear-gradient(#000 0 0)}
#f::after{inset:-2px;border-radius:29px;z-index:-1;filter:blur(16px);opacity:0;transition:opacity .45s}
#f:hover::before{opacity:.6}#f:focus-within::before{opacity:1}#f:focus-within::after{opacity:.5}
.t::before{border-radius:23px;animation-duration:3.6s}.t:hover::before{opacity:1}
#m button.ok{background:#0f0f14;box-shadow:inset 0 0 0 1px rgba(255,255,255,.1),inset 0 1px 0 rgba(255,255,255,.08);transition:transform .12s ease-out,background .15s}#m button.ok:hover{background:#1f1f28}#m button.ok:active{transform:scale(.97)}
.t{overflow:visible}
body:not(.wp-aurora)::before,body:not(.wp-aurora)::after{display:none}
body.wp-gradient{background:linear-gradient(135deg,color-mix(in srgb,var(--a) 30%,#131316),#131316 48%,color-mix(in srgb,var(--b) 34%,#131316));background-size:180% 180%;animation:gs 16s ease-in-out infinite alternate}
@keyframes gs{to{background-position:100% 100%}}
body.wp-image{background:#131316 center/cover no-repeat fixed}
.sky{position:fixed;inset:0;z-index:0;display:none}
body.wp-stars .sky{display:block}
.waves{position:fixed;left:0;right:0;bottom:0;height:34vh;z-index:0;display:none;overflow:hidden}
body.wp-waves .waves{display:block}
.waves svg{position:absolute;left:0;bottom:0;width:200%;height:100%;animation:wv 18s linear infinite}
.waves svg:nth-child(2){animation-duration:27s;animation-direction:reverse;opacity:.7;height:78%}
@keyframes wv{to{transform:translateX(-50%)}}
.clk{display:@@CLK@@;text-align:center;margin:-26px 0 30px;animation:up .8s .08s cubic-bezier(.2,.8,.2,1) both}
.clk .tm{font-size:84px;font-weight:300;letter-spacing:-2px;line-height:1}
.clk .gr{margin-top:8px;font-size:18px;color:#c9c9d3}
.clk .dt{font-size:14px;color:#8d8d98;margin-top:4px;text-transform:capitalize}
.bmt{position:relative;z-index:1;display:flex;gap:12px;align-items:center;justify-content:center;margin-top:26px;font-size:13px;animation:up .8s .4s both}
.bmt a{color:#9a9aa8;cursor:pointer;text-decoration:none;transition:color .2s}.bmt a:hover{color:#fff}.bmt i{width:3px;height:3px;border-radius:50%;background:#5a5a66}
.bms{position:relative;z-index:1;text-align:center;margin-top:8px;min-height:18px;font-size:13px;color:var(--b)}
body.noanim *,body.noanim::before,body.noanim::after{animation:none!important;transition:none!important}
</style></head><body class="wp-@@WP@@ @@NOANIM@@"><div class="bg" id="bg"></div><canvas class="sky" id="sky"></canvas><div class="waves"><svg viewBox="0 0 1200 100" preserveAspectRatio="none"><path d="M0 50 Q150 10 300 50 T600 50 T900 50 T1200 50 V100 H0Z" fill="var(--a)" fill-opacity=".16"/></svg><svg viewBox="0 0 1200 100" preserveAspectRatio="none"><path d="M0 60 Q150 25 300 60 T600 60 T900 60 T1200 60 V100 H0Z" fill="var(--b)" fill-opacity=".22"/></svg></div><div class="w"><div class="l"><img src="data:image/png;base64,@@LOGO@@" alt=""><span id="nm">Petal</span></div>
<div class="clk"><div class="tm" id="tm"></div><div class="gr" id="gr"></div><div class="dt" id="dt"></div></div>
<form id="f"><svg width="22" height="22" viewBox="0 0 22 22" fill="none" stroke="#9a9aa3" stroke-width="2" stroke-linecap="round"><circle cx="9.5" cy="9.5" r="6.5"/><path d="M14.5 14.5L19 19"/></svg>
<input id="q" autofocus autocomplete="off" spellcheck="false" placeholder="Найдите в @@ENGINE@@ или введите адрес"></form>
<div class="tiles" id="tiles"></div>
<div class="bmt"><a id="bx">Экспорт закладок</a><i></i><a id="bi">Импорт закладок</a><input type="file" id="bf" accept=".html,.htm,.json" hidden></div><div class="bms" id="bs"></div></div>
<div id="m"><form id="mf"><h3>Новая закладка</h3><input id="mn" placeholder="Название" autocomplete="off"><input id="mu" placeholder="Адрес сайта, например youtube.com" autocomplete="off" spellcheck="false"><div class="r"><button type="button" id="mc">Отмена</button><button class="ok" type="submit">Добавить</button></div></form></div>
<script>@@BGJS@@</script>
<script>
(function(){var m=document.body.className.match(/wp-(veil|ether|tunnel)/);if(!m)return;
try{if(!document.createElement('canvas').getContext('webgl2'))throw 0;}catch(e){document.body.className+=' wp-aurora';return;}
var cs=getComputedStyle(document.documentElement),A=cs.getPropertyValue('--a').trim(),B=cs.getPropertyValue('--b').trim(),an=document.body.className.indexOf('noanim')<0;
var host=document.getElementById('bg'),cv=document.createElement('canvas');host.appendChild(cv);
try{
if(m[1]==='veil')PetalBg.veil(host,cv,{color:A,speed:an?0.5:0,scale:1,rotation:0,flow:an?1:0,warp:0.12,variation:0,brightness:1,sheen:0.7,glow:0.45,grain:0.03,quality:0.5,mouseInteraction:true,mouseStrength:1,intro:an,fade:0,opacity:1,lightMode:false,paused:!an,dpr:undefined});
else if(m[1]==='ether')PetalBg.ether(host,cv,{colors:[A,B,'#ffffff'],paletteKey:A+B,intensity:1,force:20,radius:100,persistence:0.75,viscosity:0,resolution:0.5,walls:false,clickBurst:true,autoDemo:an,autoSpeed:0.5,autoIntensity:2.2,autoResumeDelay:1000,intro:an,light:false,background:[0,0,0,0],maxRatio:2});
else PetalBg.tunnel(host,cv,{cableColor:B,pulseColor:A,tunnelColor:B,tunnelOpacity:0.12,speed:an?0.1:0,flow:-1,pulseSpeed:2,pulseLength:0.28,pulseBlend:0.6,pulseWidth:1,pulseDensity:0.5,cableCount:56,thickness:0.35,rimWidth:0.15,waviness:0.3,sway:0.5,bend:0.4,size:1,centerX:0,centerY:0,glow:1,fadeNear:0.5,fadeFar:2,brightness:1,colorVariance:true,grain:false,grainIntensity:0.05,opacity:1,mouseInteraction:true,mouseStrength:0.1,clickSurge:true,intro:an,variation:0,lightMode:false,dpr:undefined});
}catch(e){document.body.className+=' wp-aurora';}
})();
</script>
<script>
document.getElementById('f').addEventListener('submit',function(e){e.preventDefault();var t=document.getElementById('q').value.trim();if(!t)return;var u;
if(/^https?:\/\//i.test(t))u=t;else if(!/\s/.test(t)&&t.indexOf('.')>0&&t.slice(-1)!=='.'&&t.indexOf('://')<0)u='https://'+t;else u='@@SEARCH@@'+encodeURIComponent(t);location.href=u;});
document.getElementById('q').focus();
var K='petal_bookmarks',D=[{n:'YouTube',u:'https://www.youtube.com/'},{n:'ВКонтакте',u:'https://vk.com/'},{n:'Яндекс',u:'https://ya.ru/'},{n:'Google',u:'https://www.google.com/'}];
function load(){try{var v=JSON.parse(localStorage.getItem(K));if(Array.isArray(v))return v;}catch(e){}return D.slice();}
function save(v){try{localStorage.setItem(K,JSON.stringify(v));}catch(e){}}
var B=load(),T=document.getElementById('tiles'),M=document.getElementById('m');
function host(u){try{return new URL(u).hostname;}catch(e){return '';}}
function draw(){T.textContent='';B.forEach(function(b,i){var a=document.createElement('a');a.className='t';a.href=b.u;a.style.animationDelay=(i*.05)+'s';
var h=host(b.u),im=document.createElement('img');im.src='https://www.google.com/s2/favicons?domain='+h+'&sz=64';im.alt='';
im.onerror=function(){var d=document.createElement('div');d.className='ph';d.textContent=(b.n||h||'?').charAt(0).toUpperCase();im.replaceWith(d);};a.appendChild(im);
var sp=document.createElement('span');sp.textContent=b.n||h;a.appendChild(sp);
var x=document.createElement('div');x.className='x';x.textContent='\u00d7';x.addEventListener('click',function(e){e.preventDefault();e.stopPropagation();B.splice(i,1);save(B);draw();});a.appendChild(x);T.appendChild(a);});
var ad=document.createElement('div');ad.className='t add';ad.style.animationDelay=(B.length*.05)+'s';var pl=document.createElement('div');pl.className='pl';pl.textContent='+';ad.appendChild(pl);var sp2=document.createElement('span');sp2.textContent='Добавить';ad.appendChild(sp2);ad.addEventListener('click',openM);T.appendChild(ad);}
function openM(){M.className='on';document.getElementById('mn').value='';document.getElementById('mu').value='';document.getElementById('mn').focus();}
function closeM(){M.className='';document.getElementById('q').focus();}
document.getElementById('mc').addEventListener('click',closeM);
M.addEventListener('mousedown',function(e){if(e.target===M)closeM();});
document.addEventListener('keydown',function(e){if(e.key==='Escape')closeM();});
document.getElementById('mf').addEventListener('submit',function(e){e.preventDefault();var u=document.getElementById('mu').value.trim(),n=document.getElementById('mn').value.trim();if(!u)return;if(!/^https?:\/\//i.test(u))u='https://'+u;B.push({n:n||host(u).replace(/^www\./,''),u:u});save(B);draw();closeM();});
draw();
(function(){var S=document.getElementById('bs');function say(t){S.textContent=t;setTimeout(function(){if(S.textContent===t)S.textContent='';},6000);}
function esc(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');}
document.getElementById('bx').addEventListener('click',function(){
var h='<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n';
B.forEach(function(b){h+='    <DT><A HREF="'+esc(b.u)+'">'+esc(b.n||b.u)+'</A>\n';});h+='</DL><p>\n';
var a=document.createElement('a');a.href=URL.createObjectURL(new Blob([h],{type:'text/html'}));a.download='petal-bookmarks.html';document.body.appendChild(a);a.click();a.remove();say('Файл petal-bookmarks.html сохранён в загрузки. Его понимают Chrome, Edge и Firefox.');});
var fi=document.getElementById('bf');document.getElementById('bi').addEventListener('click',function(){fi.value='';fi.click();});
fi.addEventListener('change',function(){var f=fi.files&&fi.files[0];if(!f)return;var r=new FileReader();r.onload=function(){var t=String(r.result||''),L=[];
try{if(/^\s*[\[{]/.test(t)){var j=JSON.parse(t);if(!Array.isArray(j))j=j.bookmarks||j.roots||[];j.forEach(function(x){if(x&&(x.u||x.url))L.push({n:x.n||x.name||x.title||'',u:x.u||x.url});});}
else{var d=new DOMParser().parseFromString(t,'text/html');d.querySelectorAll('a[href]').forEach(function(a){L.push({n:(a.textContent||'').trim(),u:a.getAttribute('href')});});}}catch(e){say('Не удалось прочитать файл.');return;}
var have={};B.forEach(function(b){have[b.u]=1;});var add=0;
L.forEach(function(x){var u=String(x.u||'').trim();if(!/^https?:\/\//i.test(u)||u.length>2000||have[u])return;have[u]=1;B.push({n:String(x.n||host(u).replace(/^www\./,'')).slice(0,80),u:u});add++;});
if(add){save(B);draw();}say(add?'Добавлено закладок: '+add+'.':'Новых закладок в файле нет.');};r.onerror=function(){say('Не удалось прочитать файл.');};r.readAsText(f);});
})();
if(Math.random()<0.0001)document.getElementById('nm').textContent='Vpital';
(function(){var NAME='@@NAME@@';function z(n){return (n<10?'0':'')+n;}
function tick(){var d=new Date(),h=d.getHours(),g=h<5?'Доброй ночи':h<12?'Доброе утро':h<18?'Добрый день':'Добрый вечер';
document.getElementById('tm').textContent=z(h)+':'+z(d.getMinutes());document.getElementById('gr').textContent=g+(NAME?', '+NAME:'');
try{document.getElementById('dt').textContent=d.toLocaleDateString('ru-RU',{weekday:'long',day:'numeric',month:'long'});}catch(e){}}
tick();setInterval(tick,10000);})();
(function(){if(document.body.className.indexOf('wp-stars')<0)return;var c=document.getElementById('sky'),x=c.getContext('2d'),S=[],n=0;
function rs(){c.width=innerWidth;c.height=innerHeight;S=[];for(var i=0;i<Math.floor(c.width*c.height/5200);i++)S.push({x:Math.random()*c.width,y:Math.random()*c.height,r:Math.random()*1.4+.3,p:Math.random()*6.28,s:Math.random()*.02+.005});}
rs();addEventListener('resize',rs);var anim=document.body.className.indexOf('noanim')<0;
function f(){x.clearRect(0,0,c.width,c.height);for(var i=0;i<S.length;i++){var t=S[i];if(anim)t.p+=t.s;x.globalAlpha=.35+.65*Math.abs(Math.sin(t.p));x.fillStyle='#fff';x.beginPath();x.arc(t.x,t.y,t.r,0,6.283);x.fill();}if(anim)requestAnimationFrame(f);}f();})();
</script></body></html>"##;

const INCOGNITO_HTML: &str = r##"<!doctype html><html lang="ru"><head><meta charset="utf-8"><title>Новая вкладка</title>
<meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="dark">
<style>
html,body{margin:0;height:100%;background:#1a1a1c;color:#e4e4e8;font-family:"Segoe UI",system-ui,sans-serif;overflow:hidden}
body::before{content:"";position:fixed;left:50%;top:-30vmax;width:70vmax;height:60vmax;margin-left:-35vmax;border-radius:50%;background:radial-gradient(closest-side,rgba(255,255,255,.07),transparent);animation:br 9s ease-in-out infinite alternate;pointer-events:none}
@keyframes br{to{transform:scale(1.25);opacity:.6}}
.w{position:relative;display:flex;flex-direction:column;align-items:center;padding-top:12vh}
.ic{width:92px;height:92px;border-radius:26px;background:linear-gradient(160deg,#3a3a3f,#26262a);display:flex;align-items:center;justify-content:center;box-shadow:0 14px 40px rgba(0,0,0,.45),inset 0 0 0 1px rgba(255,255,255,.08);animation:up .7s cubic-bezier(.2,.8,.2,1) both,fl 6s ease-in-out .7s infinite}
@keyframes fl{50%{transform:translateY(-5px)}}
@keyframes up{from{opacity:0;transform:translateY(20px)}to{opacity:1;transform:none}}
h1{margin:26px 0 8px;font-size:40px;font-weight:700;letter-spacing:-.5px;animation:up .7s .08s both}
.s{margin:0 0 38px;color:#9d9da6;font-size:16px;text-align:center;max-width:560px;animation:up .7s .14s both}
form{width:min(760px,86vw);position:relative;animation:up .7s .2s both}
input{box-sizing:border-box;width:100%;height:54px;border:0;border-radius:27px;background:#2a2a2e;color:#fff;font-size:17px;padding:0 24px 0 58px;outline:none;transition:box-shadow .25s,background .25s}
input::placeholder{color:#85858e}
input:hover{background:#303034}
input:focus{background:#303034;box-shadow:0 0 0 2px #8e8e96,0 8px 28px rgba(0,0,0,.4)}
svg.q{position:absolute;left:21px;top:16px}
.c{display:flex;gap:16px;flex-wrap:wrap;justify-content:center;margin-top:40px;width:min(760px,86vw);animation:up .7s .3s both}
.k{flex:1 1 300px;background:#222225;border:1px solid rgba(255,255,255,.06);border-radius:20px;padding:18px 20px}
.k b{display:block;margin-bottom:8px;font-size:15px;color:#fff}
.k ul{margin:0;padding-left:18px;color:#9d9da6;font-size:14px;line-height:1.7}
</style></head><body><div class="w">
<div class="ic"><svg width="56" height="56" viewBox="0 0 64 64" fill="#d2d2d8"><path d="M14 31 21 11c1-3 3-4 6-4h10c3 0 5 1 6 4l7 20z"/><rect x="6" y="31" width="52" height="5" rx="2.5"/><circle cx="21" cy="47" r="8" fill="none" stroke="#d2d2d8" stroke-width="3.5"/><circle cx="43" cy="47" r="8" fill="none" stroke="#d2d2d8" stroke-width="3.5"/><path d="M29 46q3-3 6 0" fill="none" stroke="#d2d2d8" stroke-width="3.5"/></svg></div>
<h1>Режим инкогнито</h1>
<p class="s">Petal не запоминает, что вы открывали в этой вкладке. Закройте её, и всё исчезнет.</p>
<form id="f"><svg class="q" width="22" height="22" viewBox="0 0 22 22" fill="none" stroke="#85858e" stroke-width="2" stroke-linecap="round"><circle cx="9.5" cy="9.5" r="6.5"/><path d="M14.5 14.5L19 19"/></svg>
<input id="q" autofocus autocomplete="off" spellcheck="false" placeholder="Найдите в @@ENGINE@@ или введите адрес"></form>
<div class="c"><div class="k"><b>Не сохраняется</b><ul><li>история посещений</li><li>cookie и данные сайтов</li><li>введённые в формы данные</li></ul></div>
<div class="k"><b>Остаётся видно</b><ul><li>сайтам, которые вы открываете</li><li>провайдеру и администратору сети</li><li>скачанные файлы остаются в папке</li></ul></div></div></div>
<script>
document.getElementById('f').addEventListener('submit',function(e){e.preventDefault();var t=document.getElementById('q').value.trim();if(!t)return;var u;
if(/^https?:\/\//i.test(t))u=t;else if(!/\s/.test(t)&&t.indexOf('.')>0&&t.slice(-1)!=='.'&&t.indexOf('://')<0)u='https://'+t;else u='@@SEARCH@@'+encodeURIComponent(t);location.href=u;});
document.getElementById('q').focus();
</script></body></html>"##;

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Адрес файла в папке данных в виде file://...
fn page_url(name: &str) -> String {
    let path = std::env::current_dir().unwrap_or_default().join(name);
    let p = path.to_string_lossy().replace('\\', "/");
    let mut enc = String::new();
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"/:._-~".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("file://{}{}", if enc.starts_with('/') { "" } else { "/" }, enc)
}

/// Пишет обе стартовые страницы (обычную и инкогнито) в папку данных; поисковая система вшивается в них.
fn write_pages(c: &store::BrowserCfg) {
    let fill = |t: &str| {
        t.replace("@@ENGINE@@", store::search_name(&c.search))
            .replace("@@SEARCH@@", store::search_prefix(&c.search))
    };
    let (a, b) = store::theme_colors(c);
    let img = if c.wallpaper == "image" && !c.wallpaper_img.is_empty() {
        match std::fs::read(&c.wallpaper_img) {
            Ok(d) if d.len() <= 6_000_000 => {
                let mime = if c.wallpaper_img.to_lowercase().ends_with(".png") { "image/png" } else { "image/jpeg" };
                format!("body.wp-image{{background-image:linear-gradient(rgba(10,10,14,.52),rgba(10,10,14,.52)),url(data:{mime};base64,{})}}", base64(&d))
            }
            _ => String::new(),
        }
    } else {
        String::new()
    };
    let name: String = c.user_name.chars().filter(|ch| !matches!(ch, '\'' | '"' | '\\' | '<' | '>' | '&' | '\n' | '\r')).collect();
    let html = fill(NEWTAB_HTML)
        .replace("@@LOGO@@", &base64(include_bytes!("../assets/petal.png")))
        .replace("@@BGJS@@", include_str!("../assets/bg.js"))
        .replace("</style>", &format!(":root{{--a:#{a:06x};--b:#{b:06x}}}{img}</style>"))
        .replace("@@WP@@", &c.wallpaper)
        .replace("@@NOANIM@@", if c.anim { "" } else { "noanim" })
        .replace("@@CLK@@", if c.clock { "block" } else { "none" })
        .replace("@@NAME@@", &name);
    let _ = std::fs::write("petal_newtab.html", html);
    let _ = std::fs::write("petal_incognito.html", fill(INCOGNITO_HTML));
}

/// Всё, от чего зависит вид стартовых страниц: при изменении их надо записать заново.
fn pages_key(c: &store::BrowserCfg) -> String {
    let (a, b) = store::theme_colors(c);
    format!("{}|{a:x}|{b:x}|{}|{}|{}|{}|{}", c.search, c.wallpaper, c.wallpaper_img, c.clock, c.user_name, c.anim)
}

fn newtab_url() -> &'static str {
    NEWTAB_URL.get_or_init(|| page_url("petal_newtab.html"))
}

fn incognito_url() -> &'static str {
    INCOGNITO_URL.get_or_init(|| page_url("petal_incognito.html"))
}

/// Фон под страницей, пока она грузится: тот же, что у собственных стартовых страниц, чтобы не было вспышки другого цвета.
fn page_bg(url: &str, incognito: bool) -> Color32 {
    if url.starts_with("file://") && url.ends_with("/petal_incognito.html") {
        Color32::from_rgb(26, 26, 28)
    } else if url.starts_with("file://") && url.ends_with("/petal_newtab.html") {
        Color32::from_rgb(19, 19, 22)
    } else if incognito {
        Color32::from_rgb(31, 31, 31)
    } else {
        BG
    }
}

/// Адрес для новой вкладки: стартовая страница Petal, если в настройках не выбрана своя домашняя.
fn new_tab_url(home: &str) -> String {
    let h = home.trim();
    if h.is_empty() || h == "https://www.google.com/" || h == "https://www.google.com" {
        newtab_url().to_string()
    } else {
        normalize_url(h)
    }
}

/// В адресной строке стартовую страницу показываем пустой.
fn shown_url(url: &str) -> String {
    if url.starts_with("file://") && (url.ends_with("/petal_newtab.html") || url.ends_with("/petal_incognito.html")) {
        String::new()
    } else {
        url.to_string()
    }
}

/// Ссылку http(s) оставляет как есть, «домен.ру» дополняет до https, всё остальное ищет в Google.
fn normalize_url(input: &str) -> String {
    let t = input.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        return t.to_string();
    }
    if !t.contains("://") && !t.contains(' ') && t.contains('.') && !t.ends_with('.') && !t.starts_with('.') {
        return format!("https://{t}");
    }
    format!("{}{}", search_prefix_now(), url_encode(t))
}

/// Настоящий браузер (WebView2, то есть Chromium из Edge) внутри окна. Только Windows.
#[cfg(windows)]
mod web {
    use std::sync::mpsc::Sender;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::{Arc, Mutex};
    use wry::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
    use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows};
    use super::{store, DlEvent, DL_EVENTS};

    const PIP_JS: &str = r##"(function(){if(window.__petalPip)return;window.__petalPip=1;var b=null,v=null,t=0;
function mk(){b=document.createElement('div');b.setAttribute('style','position:fixed;z-index:2147483647;width:34px;height:34px;border-radius:8px;background:rgba(30,30,34,.82);display:none;align-items:center;justify-content:center;cursor:pointer;box-shadow:0 2px 8px rgba(0,0,0,.4);');
var N='http://www.w3.org/2000/svg',g=document.createElementNS(N,'svg');g.setAttribute('width','20');g.setAttribute('height','20');g.setAttribute('viewBox','0 0 20 20');g.setAttribute('fill','none');g.setAttribute('stroke','#fff');g.setAttribute('stroke-width','1.6');
var r1=document.createElementNS(N,'rect');r1.setAttribute('x','2.5');r1.setAttribute('y','4');r1.setAttribute('width','15');r1.setAttribute('height','12');r1.setAttribute('rx','2');
var r2=document.createElementNS(N,'rect');r2.setAttribute('x','9.5');r2.setAttribute('y','9');r2.setAttribute('width','6');r2.setAttribute('height','5');r2.setAttribute('rx','1');r2.setAttribute('fill','#fff');
g.appendChild(r1);g.appendChild(r2);b.appendChild(g);
b.addEventListener('click',function(e){e.stopPropagation();e.preventDefault();if(!v)return;if(mini){unmini();return;}try{if(document.pictureInPictureElement===v){document.exitPictureInPicture();return;}if(document.pictureInPictureEnabled){v.disablePictureInPicture=false;v.requestPictureInPicture().catch(function(){goMini();});return;}}catch(x){}goMini();},true);
b.addEventListener('mousedown',function(e){e.stopPropagation();},true);}
function place(){if(!v||!v.isConnected){hide();return;}var r=v.getBoundingClientRect();if(r.width<200||r.height<110){hide();return;}
if(!b)mk();var host=document.fullscreenElement;if(!host||host===v||host.tagName==='VIDEO')host=document.documentElement;if(b.parentNode!==host)host.appendChild(b);
b.style.left=Math.max(0,Math.min(innerWidth-40,r.right-44))+'px';b.style.top=Math.max(0,r.top+10)+'px';b.style.display='flex';clearTimeout(t);if(!mini)t=setTimeout(hide,2500);}
function hide(){if(b&&!mini)b.style.display='none';}
var mini=null;
function say(m){try{window.ipc.postMessage(m);}catch(e){}}
function goMini(){if(!v||mini)return;mini={v:v,css:v.getAttribute('style')};v.style.cssText=(mini.css||'')+';position:fixed!important;left:0!important;top:0!important;width:100vw!important;height:100vh!important;max-width:none!important;max-height:none!important;z-index:2147483646!important;background:#000!important;object-fit:contain!important;transform:none!important;margin:0!important;';say('petal:pip:1');setTimeout(place,300);}
function unmini(){if(!mini)return;var m=mini;mini=null;if(m.css===null)m.v.removeAttribute('style');else m.v.setAttribute('style',m.css);say('petal:pip:0');hide();}
document.addEventListener('keydown',function(e){if(mini&&e.key==='Escape')unmini();},true);
window.addEventListener('pagehide',function(){if(mini)say('petal:pip:0');});
function find(e){var n=e.target;if(n&&n.tagName==='VIDEO')return n;var x=e.clientX,y=e.clientY,l=document.querySelectorAll('video');for(var i=0;i<l.length;i++){var r=l[i].getBoundingClientRect();if(x>=r.left&&x<=r.right&&y>=r.top&&y<=r.bottom)return l[i];}return null;}
document.addEventListener('mousemove',function(e){if(b&&e.target===b){clearTimeout(t);return;}var f=find(e);if(f){v=f;place();}else if(v){hide();}},true);
document.addEventListener('scroll',function(){if(b&&b.style.display!=='none')place();},true);
document.addEventListener('fullscreenchange',hide);
})();"##;
    /// Сообщает, можно ли идти назад и вперёд (1 = назад, 2 = вперёд), чтобы стрелки гасли, когда идти некуда.
    /// Следит за звуком: ищет играющие video/audio (и в iframe), сообщает «петал:aud:1/0» и умеет глушить.
    const AUDIO_JS: &str = r#"(function(){
var isTop=window===window.top,muted=false,last=-1,kids=0,seen=[];
function media(){return document.querySelectorAll('video,audio');}
function apply(){var n=media();for(var i=0;i<n.length;i++){var e=n[i];if(muted){if(!e.muted){e.__pm=1;e.muted=true;}}else if(e.__pm){e.muted=false;e.__pm=0;}}}
function own(){var n=media();for(var i=0;i<n.length;i++){var e=n[i];if(!e.paused&&!e.ended&&e.readyState>2&&e.volume>0&&(!e.muted||e.__pm))return 1;}return 0;}
function down(){var f=document.querySelectorAll('iframe');for(var i=0;i<f.length;i++){try{f[i].contentWindow.postMessage({petalMute:muted},'*');}catch(e){}}}
function tick(){
 apply();
 var now=Date.now(),k=0;for(var i=0;i<seen.length;i++){if(now-seen[i].t<1500&&seen[i].v)k=1;}kids=k;
 var v=(own()||kids)?1:0;
 if(muted)down();
 if(isTop){if(v!==last){last=v;try{window.ipc.postMessage('petal:aud:'+v);}catch(e){}}}
 else{try{parent.postMessage({petalAud:v},'*');}catch(e){}}
}
window.addEventListener('message',function(ev){
 var d=ev.data;if(!d||typeof d!=='object')return;
 if('petalAud' in d){var f=null;for(var i=0;i<seen.length;i++){if(seen[i].s===ev.source){f=seen[i];break;}}if(!f){f={s:ev.source,t:0,v:0};seen.push(f);}f.t=Date.now();f.v=d.petalAud?1:0;}
 else if('petalMute' in d&&!isTop&&ev.source===parent){muted=!!d.petalMute;apply();down();}
});
if(isTop){window.__petalMute=function(b){muted=!!b;apply();down();tick();};}
setInterval(tick,500);
})();"#;
    const NAV_JS: &str = "(function(){if(window!==window.top)return;var l=-1;function r(){var v=0;try{if(window.navigation){v=(navigation.canGoBack?1:0)|(navigation.canGoForward?2:0);}else{v=(history.length>1?1:0)|2;}}catch(e){}if(v!==l){l=v;try{window.ipc.postMessage('petal:nav:'+v);}catch(e){}}}setInterval(r,400);r();})();";
    /// Сообщает сайтам «не отслеживайте меня» (Do Not Track и Global Privacy Control).
    const PRIVACY_JS: &str = "(function(){try{Object.defineProperty(navigator,'doNotTrack',{get:function(){return '1';}});Object.defineProperty(navigator,'globalPrivacyControl',{get:function(){return true;}});}catch(e){}})();";
    const FULLSCREEN_JS: &str = "(function(){function s(){try{window.ipc.postMessage(document.fullscreenElement?'petal:fs:1':'petal:fs:0');}catch(e){}}document.addEventListener('fullscreenchange',s);document.addEventListener('webkitfullscreenchange',s);})();";

    pub struct Browser {
        webview: WebView,
    }

    fn rect(b: [f32; 4]) -> Rect {
        Rect {
            position: Position::Physical(PhysicalPosition::new(b[0].round() as i32, b[1].round() as i32)),
            size: Size::Physical(PhysicalSize::new(b[2].round().max(1.0) as u32, b[3].round().max(1.0) as u32)),
        }
    }

    impl Browser {
        /// `bounds` = [x, y, ширина, высота] в физических пикселях окна.
        /// Название страницы пишется в `title`, а ссылки «в новом окне» приходят в `new_tabs`.
        pub fn new(
            frame: &eframe::Frame,
            url: &str,
            bounds: [f32; 4],
            title: Arc<Mutex<String>>,
            new_tabs: Sender<String>,
            incognito: bool,
            fullscreen: Arc<AtomicBool>,
            pip: Arc<AtomicBool>,
            nav: Arc<AtomicU8>,
            audio: Arc<AtomicBool>,
        ) -> Result<Self, String> {
            let cfg = super::FROZEN.get().cloned().unwrap_or_default();
            // В инкогнито с автоочисткой загрузки не попадают в общий список.
            let hide_dl = incognito && cfg.wipe_incognito;
            let mut builder = WebViewBuilder::new()
                // Страница сообщает, когда её элемент (например, видео на YouTube) уходит на весь экран.
                .with_initialization_script(FULLSCREEN_JS)
                .with_initialization_script(PIP_JS)
                .with_initialization_script(NAV_JS)
                .with_initialization_script(AUDIO_JS)
                .with_ipc_handler(move |req| match req.body().as_str() {
                    "petal:fs:1" => fullscreen.store(true, Ordering::Relaxed),
                    "petal:fs:0" => fullscreen.store(false, Ordering::Relaxed),
                    "petal:pip:1" => pip.store(true, Ordering::Relaxed),
                    "petal:pip:0" => pip.store(false, Ordering::Relaxed),
                    "petal:aud:1" => audio.store(true, Ordering::Relaxed),
                    "petal:aud:0" => audio.store(false, Ordering::Relaxed),
                    m => {
                        if let Some(n) = m.strip_prefix("petal:nav:").and_then(|n| n.parse::<u8>().ok()) {
                            nav.store(n, Ordering::Relaxed);
                        }
                    }
                })
                .with_incognito(incognito)
                // Тёмный фон до загрузки страницы и тёмная тема для сайтов, которые её поддерживают.
                .with_background_color({ let c = super::page_bg(&url, incognito); (c.r(), c.g(), c.b(), 255) })
                .with_theme(if cfg.dark_sites { wry::Theme::Dark } else { wry::Theme::Light })
                .with_additional_browser_args(store::browser_args(&cfg))
                .with_download_started_handler(move |url, path| {
                    // Своя папка для загрузок, если выбрана в настройках.
                    if let Ok(dir) = super::DL_DIR.lock() {
                        if !dir.trim().is_empty() {
                            if let Some(name) = path.file_name().map(|n| n.to_owned()) {
                                let d = std::path::PathBuf::from(dir.trim());
                                if std::fs::create_dir_all(&d).is_ok() {
                                    *path = d.join(name);
                                }
                            }
                        }
                    }
                    if !hide_dl {
                        if let Ok(mut q) = DL_EVENTS.lock() {
                            q.push(DlEvent::Started { url, path: path.clone() });
                        }
                    }
                    true
                })
                .with_download_completed_handler(move |url, path, ok| {
                    if hide_dl {
                        return;
                    }
                    if let Ok(mut q) = DL_EVENTS.lock() {
                        q.push(DlEvent::Done { url, path, ok });
                    }
                })
                .with_url(url)
                .with_bounds(rect(bounds))
                .with_document_title_changed_handler(move |t| {
                    if let Ok(mut g) = title.lock() {
                        *g = t;
                    }
                })
                .with_new_window_req_handler(move |url, features| {
                    // Окно с заданным размером (вход через Google, Facebook и т. п.) пусть остаётся окном,
                    // иначе оно теряет связь со страницей. Обычные ссылки «в новом окне» открываем вкладкой.
                    if features.size.is_some() {
                        if super::BLOCK_POPUPS.load(Ordering::Relaxed) {
                            NewWindowResponse::Deny
                        } else {
                            NewWindowResponse::Allow
                        }
                    } else {
                        let _ = new_tabs.send(url);
                        NewWindowResponse::Deny
                    }
                })
                ;
            if cfg.extensions {
                let dir = std::env::current_dir().unwrap_or_default().join("extensions");
                let _ = std::fs::create_dir_all(&dir);
                builder = builder.with_browser_extensions_enabled(true).with_extensions_path(dir);
            }
            let ua = store::user_agent(&cfg.ua);
            if !ua.is_empty() {
                builder = builder.with_user_agent(ua);
            }
            if cfg.privacy_signals {
                builder = builder.with_initialization_script(PRIVACY_JS);
            }
            let webview = builder.build_as_child(frame).map_err(|e| e.to_string())?;
            let _ = webview.zoom(super::ZOOM.load(Ordering::Relaxed) as f64 / 100.0);
            Ok(Browser { webview })
        }
        pub fn navigate(&self, url: &str) {
            let _ = self.webview.load_url(url);
        }
        pub fn back(&self) {
            let _ = self.webview.evaluate_script("history.back()");
        }
        pub fn forward(&self) {
            let _ = self.webview.evaluate_script("history.forward()");
        }
        pub fn reload(&self) {
            let _ = self.webview.reload();
        }
        pub fn url(&self) -> Option<String> {
            self.webview.url().ok()
        }
        pub fn set_bounds(&self, bounds: [f32; 4]) {
            let _ = self.webview.set_bounds(rect(bounds));
        }
        pub fn focus_parent(&self) {
            let _ = self.webview.focus_parent();
        }
        /// Масштаб страницы в процентах.
        pub fn set_zoom(&self, percent: u32) {
            let _ = self.webview.zoom(percent as f64 / 100.0);
        }
        /// Стирает cookie, кэш и прочие данные сайтов (из аккаунтов придётся выйти).
        pub fn clear_data(&self) {
            let _ = self.webview.clear_all_browsing_data();
        }
        pub fn eval(&self, js: &str) {
            let _ = self.webview.evaluate_script(js);
        }
    }
}

#[cfg(not(windows))]
mod web {
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};

    pub struct Browser;

    impl Browser {
        pub fn new(
            _: &eframe::Frame,
            _: &str,
            _: [f32; 4],
            _: Arc<Mutex<String>>,
            _: Sender<String>,
            _: bool,
            _: Arc<std::sync::atomic::AtomicBool>,
            _: Arc<std::sync::atomic::AtomicBool>,
            _: Arc<std::sync::atomic::AtomicU8>,
            _: Arc<std::sync::atomic::AtomicBool>,
        ) -> Result<Self, String> {
            Err("Встроенный браузер работает только в Windows.".to_string())
        }
        pub fn navigate(&self, _: &str) {}
        pub fn back(&self) {}
        pub fn forward(&self) {}
        pub fn reload(&self) {}
        pub fn url(&self) -> Option<String> {
            None
        }
        pub fn set_bounds(&self, _: [f32; 4]) {}
        pub fn focus_parent(&self) {}
        pub fn set_zoom(&self, _: u32) {}
        pub fn clear_data(&self) {}
        pub fn eval(&self, _: &str) {}
    }
}

/// Название вкладки: заголовок страницы, а пока его нет, адрес сайта.
fn tab_label(title: &str, url: &str) -> String {
    let t = title.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    host_of(url).unwrap_or_else(|| "Новая вкладка".to_string())
}

/// Значок сайта: берём из сервиса значков по домену. Пока грузится или недоступен, рисуется буква.
fn favicon(ctx: &egui::Context, url: &str) -> Option<egui::load::SizedTexture> {
    let host = host_of(url)?;
    if host.contains(':') || !host.contains('.') {
        return None;
    }
    let uri = format!("https://www.google.com/s2/favicons?domain={host}&sz=64");
    match ctx.try_load_texture(&uri, egui::TextureOptions::LINEAR, egui::load::SizeHint::Size(32, 32)) {
        Ok(egui::load::TexturePoll::Ready { texture }) => Some(texture),
        _ => None,
    }
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    let host = rest.split('/').next()?.trim_start_matches("www.");
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

// ───────────────────────── Саундпад: файлы ─────────────────────────
//
// sounds/                 звуки без группы
// sounds/<группа>/        звуки группы (группа это обычная папка)
// sounds/hotkeys.txt      назначенные клавиши: «путь<TAB>клавиша»

const SOUNDS_DIR: &str = "sounds";
const HOTKEYS_FILE: &str = "hotkeys.txt";
const AUDIO_EXTS: [&str; 4] = ["mp3", "wav", "ogg", "flac"];

#[derive(Clone)]
struct Sound {
    name: String,
    group: String,
    path: PathBuf,
    /// Путь относительно папки sounds через «/»: однозначно называет звук.
    rel: String,
}

fn is_audio_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn sound_name(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("звук").to_string()
}

/// Имя файла или папки без символов, запрещённых в Windows.
fn clean_name(s: &str) -> String {
    let s: String = s
        .trim()
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    s.trim().trim_end_matches('.').trim().chars().take(80).collect()
}

fn rel_path(dir: &Path, p: &Path) -> String {
    p.strip_prefix(dir)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn audio_files_in(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file() && is_audio_file(p)).collect())
        .unwrap_or_default()
}

/// Группы: подпапки папки sounds, по алфавиту.
fn list_groups(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|g| g.to_lowercase());
    v
}

/// Все звуки: сначала без группы, потом по группам, внутри по алфавиту.
fn list_sounds(dir: &Path) -> Vec<Sound> {
    let make = |group: &str, path: PathBuf| Sound {
        name: sound_name(&path),
        group: group.to_string(),
        rel: rel_path(dir, &path),
        path,
    };
    let mut v: Vec<Sound> = audio_files_in(dir).into_iter().map(|p| make("", p)).collect();
    for g in list_groups(dir) {
        v.extend(audio_files_in(&dir.join(&g)).into_iter().map(|p| make(&g, p)));
    }
    v.sort_by_key(|s| (s.group.to_lowercase(), s.name.to_lowercase()));
    v
}

fn group_dir(dir: &Path, group: &str) -> PathBuf {
    if group.is_empty() {
        dir.to_path_buf()
    } else {
        dir.join(clean_name(group))
    }
}

/// Свободное имя файла в папке: «звук.mp3», затем «звук (2).mp3» и так далее.
fn unique_dest(dir: &Path, file_name: &str) -> PathBuf {
    let first = dir.join(file_name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(file_name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("звук");
    let ext = p.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Проверяет, что файл правда читается как аудио, и копирует его в папку группы.
fn import_sound(src: &Path, dir: &Path, group: &str) -> Result<String, String> {
    let file_name = src.file_name().and_then(|s| s.to_str()).unwrap_or("звук").to_string();
    if !is_audio_file(src) {
        return Err(format!("{file_name}: неподдерживаемый формат (подходят mp3, wav, ogg, flac)"));
    }
    let f = File::open(src).map_err(|e| format!("{file_name}: не удалось открыть ({e})"))?;
    rodio::Decoder::try_from(f).map_err(|_| format!("{file_name}: файл повреждён или не является аудио"))?;
    let target = group_dir(dir, group);
    std::fs::create_dir_all(&target).map_err(|e| format!("Не удалось создать папку ({e})"))?;
    let dest = unique_dest(&target, &file_name);
    std::fs::copy(src, &dest).map_err(|e| format!("{file_name}: не удалось скопировать ({e})"))?;
    Ok(sound_name(&dest))
}

/// Папка целиком: все её звуки попадают в группу с именем папки.
fn import_folder(src: &Path, dir: &Path) -> (usize, Vec<String>) {
    let mut group = clean_name(src.file_name().and_then(|s| s.to_str()).unwrap_or(""));
    if group.is_empty() {
        group = "Импорт".to_string();
    }
    let mut files = audio_files_in(src);
    files.sort();
    let (mut ok, mut errors) = (0, Vec::new());
    for f in files {
        match import_sound(&f, dir, &group) {
            Ok(_) => ok += 1,
            Err(e) => errors.push(e),
        }
    }
    (ok, errors)
}

fn create_group(dir: &Path, name: &str) -> Result<String, String> {
    let g = clean_name(name);
    if g.is_empty() {
        return Err("Введите название группы".to_string());
    }
    std::fs::create_dir_all(dir.join(&g)).map_err(|e| format!("Не удалось создать группу ({e})"))?;
    Ok(g)
}

fn rename_sound(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let name = clean_name(new_name);
    if name.is_empty() {
        return Err("Название не может быть пустым".to_string());
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mp3");
    let dest = path.with_file_name(format!("{name}.{ext}"));
    if dest == path {
        return Ok(dest);
    }
    // В Windows «abc» и «ABC» это один и тот же файл: смена регистра разрешена.
    let same_file = dest.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
    if dest.exists() && !same_file {
        return Err(format!("Звук «{name}» уже есть в этой группе"));
    }
    std::fs::rename(path, &dest).map_err(|e| format!("Не удалось переименовать ({e})"))?;
    Ok(dest)
}

fn move_sound(path: &Path, dir: &Path, group: &str) -> Result<PathBuf, String> {
    let target = group_dir(dir, group);
    if path.parent() == Some(target.as_path()) {
        return Ok(path.to_path_buf());
    }
    std::fs::create_dir_all(&target).map_err(|e| format!("Не удалось создать группу ({e})"))?;
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("звук");
    let dest = unique_dest(&target, name);
    std::fs::rename(path, &dest).map_err(|e| format!("Не удалось переместить ({e})"))?;
    Ok(dest)
}

fn load_hotkeys(dir: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(dir.join(HOTKEYS_FILE))
        .map(|s| {
            s.lines()
                .filter_map(|l| l.split_once('\t'))
                .map(|(a, b)| (a.to_string(), b.trim().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn save_hotkeys(dir: &Path, map: &HashMap<String, String>) {
    let mut lines: Vec<String> = map.iter().map(|(k, v)| format!("{k}\t{v}")).collect();
    lines.sort();
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(HOTKEYS_FILE), lines.join("\n"));
}

/// «Num1» показываем как «1».
fn key_label(name: &str) -> &str {
    name.strip_prefix("Num").unwrap_or(name)
}

fn group_color(g: &str) -> Color32 {
    if g.is_empty() {
        MUTED
    } else {
        let h = g.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
        WHEEL_PALETTE[h % WHEEL_PALETTE.len()]
    }
}

fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

fn sounds_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(SOUNDS_DIR)
}

// ───────────────────────── Мелкие помощники интерфейса ─────────────────────────

const KEYS: [[&str; 5]; 5] = [
    ["7", "8", "9", "/", "("],
    ["4", "5", "6", "*", ")"],
    ["1", "2", "3", "-", "^"],
    ["0", ".", "+", "pi", "sqrt"],
    ["C", "Стереть", "e", "ln", "="],
];

/// Клавиша калькулятора: цвет зависит от вида клавиши, при наведении подсвечивается и «вдавливается».
fn calc_key(ui: &mut egui::Ui, label: &str, w: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 48.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let t = hover_t(ui, &resp);
    let down = if resp.is_pointer_button_down_on() { 1.5 } else { 0.0 };
    let r = rect.translate(Vec2::new(0.0, down - 1.0 * t));
    let p = ui.painter();
    let (base, hot, ink) = match label {
        "=" => (accent_a(), lit(accent_a()), Color32::WHITE),
        "C" | "Стереть" => (Color32::from_rgb(78, 38, 62), Color32::from_rgb(120, 52, 84), Color32::from_rgb(255, 150, 170)),
        "+" | "-" | "*" | "/" | "^" | "(" | ")" => (Color32::from_rgb(52, 46, 96), Color32::from_rgb(80, 70, 150), link()),
        "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "." => (Color32::from_rgb(44, 44, 58), Color32::from_rgb(66, 66, 88), Color32::WHITE),
        _ => (Color32::from_rgb(36, 40, 62), Color32::from_rgb(54, 62, 100), Color32::from_rgb(150, 190, 255)),
    };
    if label == "=" {
        grad_rrect(p, r, 15.0, mix(accent_a(), hot, t), mix(accent_b(), lit(accent_b()), t), false);
    } else {
        p.rect_filled(r.translate(Vec2::new(0.0, 2.0)), CornerRadius::same(15), Color32::from_rgba_unmultiplied(0, 0, 0, 70));
        p.rect_filled(r, CornerRadius::same(15), mix(base, hot, t));
    }
    let size = if label.chars().count() > 3 { 14.0 } else { 18.0 };
    p.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(size), ink);
    resp
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = FIELD_BG;
    v.selection.bg_fill = alpha(accent_b(), 110);
    v.selection.stroke = Stroke::new(1.0_f32, TEXT);
    let radius = CornerRadius::same(12);
    v.widgets.noninteractive.corner_radius = radius;
    v.widgets.inactive.corner_radius = radius;
    v.widgets.hovered.corner_radius = radius;
    v.widgets.active.corner_radius = radius;
    v.widgets.inactive.weak_bg_fill = FIELD_BG;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(46, 46, 58);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(58, 58, 72);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 18));
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 60));
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 90));
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(10.0, 10.0);
        s.spacing.button_padding = Vec2::new(14.0, 7.0);
    });
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let r = Frame::new()
        .fill(CARD_BG)
        .corner_radius(CornerRadius::same(20))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 16)))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    // Светлая кромка сверху и тонкая цветная линия слева: карточка читается как отдельная плоскость.
    let rc = r.response.rect;
    let p = ui.painter();
    p.line_segment([rc.left_top() + Vec2::new(20.0, 1.0), rc.right_top() + Vec2::new(-20.0, 1.0)], Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 30)));
    ui.add_space(8.0);
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (bar, _) = ui.allocate_exact_size(Vec2::new(4.0, 18.0), Sense::hover());
        grad_rrect(ui.painter(), bar, 2.0, accent_a(), accent_b(), true);
        ui.label(RichText::new(title).size(17.0).strong().color(TEXT));
    });
    ui.add_space(6.0);
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(13.0).color(MUTED));
}

/// Выпадающий список «подпись + варианты»; `cur` хранит идентификатор выбранного пункта.
fn combo(ui: &mut egui::Ui, id: &str, label: &str, opts: &[(&str, &str)], cur: &mut String) {
    ui.label(RichText::new(label).color(MUTED));
    let shown = opts.iter().find(|o| o.0 == cur.as_str()).map(|o| o.1).unwrap_or("—");
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(260.0).show_ui(ui, |ui| {
        for (k, name) in opts {
            ui.selectable_value(cur, k.to_string(), *name);
        }
    });
}

fn header(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.label(RichText::new(title).size(30.0).strong().color(TEXT));
    let (bar, _) = ui.allocate_exact_size(Vec2::new(54.0, 4.0), Sense::hover());
    grad_rrect(ui.painter(), bar, 2.0, accent_a(), accent_b(), false);
    ui.add_space(2.0);
    ui.label(RichText::new(sub).size(14.0).color(MUTED));
    ui.add_space(12.0);
}

#[derive(Clone, Copy)]
enum NavIcon {
    Back,
    Forward,
    Reload,
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Скруглённый прямоугольник с плавным градиентом (по горизонтали или по вертикали).
fn grad_rrect(p: &egui::Painter, rect: egui::Rect, r: f32, c0: Color32, c1: Color32, vertical: bool) {
    let r = r.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (egui::pos2(rect.right() - r, rect.top() + r), -90.0_f32),
        (egui::pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (egui::pos2(rect.left() + r, rect.bottom() - r), 90.0),
        (egui::pos2(rect.left() + r, rect.top() + r), 180.0),
    ];
    let color_at = |q: egui::Pos2| {
        let t = if vertical { (q.y - rect.top()) / rect.height().max(1.0) } else { (q.x - rect.left()) / rect.width().max(1.0) };
        mix(c0, c1, t)
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.center(), color_at(rect.center()));
    for (c, start) in corners {
        for k in 0..=12 {
            let a = (start + 90.0 * k as f32 / 12.0).to_radians();
            let q = c + Vec2::new(a.cos(), a.sin()) * r;
            mesh.colored_vertex(q, color_at(q));
        }
    }
    let n = 52u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Мягкое круглое пятно света: в центре заданный цвет, к краю прозрачное.
fn radial_glow(p: &egui::Painter, center: egui::Pos2, radius: f32, color: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, color);
    let n = 32u32;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(center + Vec2::new(a.cos(), a.sin()) * radius, Color32::TRANSPARENT);
    }
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Выпадающая карточка, которая плавно вырастает из кнопки. Возвращает видимый прямоугольник.
fn popup_card(ctx: &egui::Context, key: &str, open: bool, pos: egui::Pos2, width: f32, up: bool, add: impl FnOnce(&mut egui::Ui)) -> Option<egui::Rect> {
    let t = ctx.abt(egui::Id::new((key, "t")), open, 0.26);
    if t < 0.002 {
        return None;
    }
    let e = t * t * (3.0 - 2.0 * t);
    let hid = egui::Id::new((key, "h"));
    let full: f32 = ctx.data(|d| d.get_temp(hid)).unwrap_or(260.0);
    let top = if up { pos - Vec2::new(0.0, full * e - 12.0 * (1.0 - e)) } else { pos - Vec2::new(0.0, 12.0 * (1.0 - e)) };
    let vis = egui::Rect::from_min_size(top, Vec2::new(width + 2.0, (full * e).max(1.0)));
    egui::Area::new(egui::Id::new((key, "area"))).order(egui::Order::Foreground).fixed_pos(top).show(ctx, |ui| {
        ui.set_clip_rect(vis);
        ui.set_opacity(e);
        let r = Frame::new()
            .fill(CARD_BG)
            .corner_radius(CornerRadius::same(18))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 26)))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(width - 24.0);
                add(ui);
            });
        ctx.data_mut(|d| d.insert_temp(hid, r.response.rect.height()));
    });
    Some(vis)
}

/// Плавная «степень наведения» 0..1 для кнопки.
fn hover_t(ui: &egui::Ui, resp: &egui::Response) -> f32 {
    ui.ctx().abt(resp.id.with("hov"), resp.hovered(), 0.15)
}

/// Стеклянная кнопка-кружок: полупрозрачная заливка, тонкая рамка; при наведении светлеет и обводится цветом темы.
fn glass_base(p: &egui::Painter, rect: egui::Rect, radius: f32, t: f32, o: f32) {
    // Графитовая кнопка с объёмом: мягкая тень, вертикальный градиент, светлая кромка сверху. Без свечения.
    let r = CornerRadius::same(radius as u8);
    let glow = t.max(o);
    let wide = p.with_clip_rect(p.clip_rect().union(rect.expand(12.0)));
    if rect.height() >= 38.0 {
        wide.add(egui::epaint::Shadow { offset: [0, 2], blur: 7, spread: 0, color: Color32::from_black_alpha((80.0 + 50.0 * t) as u8) }.as_shape(rect, r));
    }
    let top = mix(Color32::from_rgb(33, 33, 42), Color32::from_rgb(46, 46, 58), t);
    let bottom = mix(Color32::from_rgb(12, 12, 16), Color32::from_rgb(20, 20, 27), t);
    if rect.height() >= 38.0 {
        grad_rrect(p, rect, radius, top, bottom, true);
    } else {
        // На мелких кнопках сетка градиента даёт зубцы по краю, поэтому там ровная заливка.
        p.rect_filled(rect, r, mix(top, bottom, 0.55));
    }
    if o > 0.01 {
        p.rect_filled(rect, r, alpha(accent_b(), (34.0 * o) as u8));
    }
    let inset = radius;
    if rect.height() > 2.0 * radius + 1.0 {
        p.line_segment(
            [rect.left_top() + Vec2::new(inset, 1.0), rect.right_top() + Vec2::new(-inset, 1.0)],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, (26.0 + 26.0 * t) as u8)),
        );
    }
    p.rect_stroke(rect, r, Stroke::new(1.0_f32, mix(Color32::from_rgba_unmultiplied(255, 255, 255, 20), alpha(link(), 160), glow)), egui::StrokeKind::Inside);
}

/// Тихая стеклянная кнопка: лёгкая светлая заливка и тонкая рамка, без тени и тёмной плашки.
/// `tint` (0..1) подкрашивает кнопку цветом темы; на наведении заливка и рамка проявляются.
fn quiet_base(p: &egui::Painter, rect: egui::Rect, radius: f32, t: f32, tint: f32) {
    // Нейтральное матовое стекло: без цветных оттенков. Верхняя кромка светлее, нижняя чуть темнее.
    let r = CornerRadius::same(radius as u8);
    let base = 7.0 + 11.0 * t + 12.0 * tint;
    p.rect_filled(rect, r, Color32::from_rgba_unmultiplied(255, 255, 255, base as u8));
    let inner = rect.shrink(1.0);
    let rr = CornerRadius::same((radius - 1.0).max(0.0) as u8);
    // Нижняя половина затемнена: даёт лёгкий объём.
    let low = egui::Rect::from_min_max(egui::pos2(inner.left(), inner.center().y), inner.right_bottom());
    if rect.width() < 60.0 {
        p.rect_filled(low, CornerRadius { nw: 0, ne: 0, sw: rr.sw, se: rr.se }, Color32::from_black_alpha(22));
    }
    let inset = radius * 0.9;
    p.line_segment(
        [rect.left_top() + Vec2::new(inset, 1.0), rect.right_top() + Vec2::new(-inset, 1.0)],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, (38.0 + 34.0 * t) as u8)),
    );
    p.rect_stroke(rect, r, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, (22.0 + 40.0 * t + 24.0 * tint) as u8)), egui::StrokeKind::Inside);
}

/// Приподнятая кнопка, заметная как кнопка с розой: плотная заливка-градиент, цветная «губа» снизу,
/// светлая рамка; при наведении поднимается и светлеет. Возвращает прямоугольник с учётом подъёма.
fn raised_base(p: &egui::Painter, rect0: egui::Rect, radius: f32, t: f32, o: f32, enabled: bool) -> egui::Rect {
    let rect = rect0.translate(Vec2::new(0.0, -1.5 * t));
    let k = if enabled { 1.0 } else { 0.38 };
    let wide = p.with_clip_rect(p.clip_rect().union(rect0.expand(10.0)));
    // «Губа» чуть уже кнопки по бокам, чтобы низ выглядел ровно и не вылезал за края.
    wide.rect_filled(rect0.translate(Vec2::new(0.0, 3.0)).shrink2(Vec2::new(2.5, 0.0)), CornerRadius::same(radius as u8), alpha(accent_b(), ((26.0 + 48.0 * t + 40.0 * o) * k) as u8));
    let hi = mix(Color32::from_rgb(86, 80, 150), lit(accent_b()), 0.25 + 0.25 * t + 0.3 * o);
    let lo = mix(Color32::from_rgb(40, 36, 86), accent_b(), 0.12 + 0.2 * t + 0.3 * o);
    let hi = mix(Color32::from_rgb(38, 36, 60), hi, k);
    let lo = mix(Color32::from_rgb(24, 22, 40), lo, k);
    grad_rrect(&wide, rect, radius, hi, lo, true);
    p.rect_stroke(rect, CornerRadius::same(radius as u8), Stroke::new(1.3_f32, Color32::from_rgba_unmultiplied(255, 255, 255, ((52.0 + 120.0 * o + 40.0 * t) * k) as u8)), egui::StrokeKind::Inside);
    rect
}

/// Линия со скруглёнными концами.
/// Значок динамика: волны, когда звук идёт, и крестик, когда выключен.
fn speaker_icon(p: &egui::Painter, c: egui::Pos2, color: Color32, muted: bool) {
    let st = Stroke::new(1.6_f32, color);
    let body = vec![
        c + Vec2::new(-7.0, -2.6),
        c + Vec2::new(-4.0, -2.6),
        c + Vec2::new(0.0, -6.0),
        c + Vec2::new(0.0, 6.0),
        c + Vec2::new(-4.0, 2.6),
        c + Vec2::new(-7.0, 2.6),
    ];
    p.add(egui::Shape::convex_polygon(body, color, Stroke::NONE));
    if muted {
        round_line(p, c + Vec2::new(3.5, -3.0), c + Vec2::new(8.5, 3.0), st);
        round_line(p, c + Vec2::new(8.5, -3.0), c + Vec2::new(3.5, 3.0), st);
    } else {
        for (k, r) in [3.5_f32, 7.0].into_iter().enumerate() {
            let pts: Vec<egui::Pos2> = (0..=8)
                .map(|n| {
                    let a = (-48.0 + 12.0 * n as f32).to_radians();
                    c + Vec2::new(1.0 + a.cos() * r, a.sin() * r)
                })
                .collect();
            p.add(egui::Shape::line(pts, Stroke::new(1.5_f32, alpha(color, if k == 0 { 255 } else { 190 }))));
        }
    }
}

fn round_line(p: &egui::Painter, a: egui::Pos2, b: egui::Pos2, st: Stroke) {
    p.line_segment([a, b], st);
    p.circle_filled(a, st.width / 2.0, st.color);
    p.circle_filled(b, st.width / 2.0, st.color);
}

// ───────────────────────── Бегущий свет по ободку (как у GlowButton) ─────────────────────────

/// Длина ободка скруглённого прямоугольника.
fn rim_len(rect: egui::Rect, r: f32) -> f32 {
    2.0 * (rect.width() - 2.0 * r).max(0.0) + 2.0 * (rect.height() - 2.0 * r).max(0.0) + std::f32::consts::TAU * r
}

/// Точка на ободке: s отсчитывается по часовой стрелке от середины верхней стороны.
fn rim_point(rect: egui::Rect, r: f32, s: f32) -> egui::Pos2 {
    use std::f32::consts::{FRAC_PI_2, PI};
    let lx = (rect.width() - 2.0 * r).max(0.0);
    let ly = (rect.height() - 2.0 * r).max(0.0);
    let arc = FRAC_PI_2 * r;
    let total = rim_len(rect, r);
    let mut s = s.rem_euclid(total);
    let (l, t, rt, b) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    let c = rect.center().x;
    let pieces = [lx / 2.0, arc, ly, arc, lx, arc, ly, arc, lx / 2.0];
    for (k, len) in pieces.iter().enumerate() {
        if s <= *len || k == pieces.len() - 1 {
            return match k {
                0 => egui::pos2(c + s, t),
                1 => egui::pos2(rt - r, t + r) + Vec2::angled(-FRAC_PI_2 + s / r) * r,
                2 => egui::pos2(rt, t + r + s),
                3 => egui::pos2(rt - r, b - r) + Vec2::angled(s / r) * r,
                4 => egui::pos2(rt - r - s, b),
                5 => egui::pos2(l + r, b - r) + Vec2::angled(FRAC_PI_2 + s / r) * r,
                6 => egui::pos2(l, b - r - s),
                7 => egui::pos2(l + r, t + r) + Vec2::angled(PI + s / r) * r,
                _ => egui::pos2(l + r + s, t),
            };
        }
        s -= len;
    }
    rect.center_top()
}

/// Положение света на круге: два оборота за цикл с небольшим «разгоном», как у GlowButton.
fn ring_phase(x: f32) -> f32 {
    let x = x.rem_euclid(1.0);
    2.0 * x + 0.07 * (std::f32::consts::TAU * 2.0 * x).sin()
}

/// Свет по ободку: яркая белая «искра» с хвостом и цветной огонёк впереди, три слоя (ореол, свечение, ядро).
fn glow_ring(p: &egui::Painter, rect: egui::Rect, radius: f32, phase: f32, k: f32) {
    if k < 0.02 {
        return;
    }
    let r = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let total = rim_len(rect, r);
    let h = rect.height().max(1.0);
    let n = ((total / 1.4) as usize).clamp(40, 360);
    let head = phase.rem_euclid(1.0) * total;
    let lead = total * 0.27;
    let wrap = |d: f32| {
        let d = d.rem_euclid(total);
        if d > total / 2.0 { d - total } else { d }
    };
    let (ca, cb) = (accent_a(), accent_b());
    let mut pts = Vec::with_capacity(n + 1);
    let mut cols: Vec<[f32; 4]> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let s = i as f32 / n as f32 * total;
        pts.push(rim_point(rect, r, s));
        let dw = wrap(s - head) / h;
        let w = if dw < 0.0 { (dw / 0.85).exp() } else { (-(dw / 0.16).powi(2)).exp() };
        let dc = wrap(s - head - lead) / h;
        let c = if dc < 0.0 { (dc / 1.5).exp() } else { (-(dc / 0.4).powi(2)).exp() };
        let f = (dc / 1.4 + 0.5).clamp(0.0, 1.0);
        let tint = [ca.r() as f32 + (cb.r() as f32 - ca.r() as f32) * f, ca.g() as f32 + (cb.g() as f32 - ca.g() as f32) * f, ca.b() as f32 + (cb.b() as f32 - ca.b() as f32) * f];
        let rgb = [0, 1, 2].map(|j| (255.0 * w * 0.95 + tint[j] * c * 1.15).min(255.0) * k);
        let a = rgb[0].max(rgb[1]).max(rgb[2]);
        cols.push([rgb[0], rgb[1], rgb[2], a]);
    }
    // Кружки вместо отрезков: слои смыкаются без «зубцов».
    for (rad, mult) in [(3.4_f32, 0.15_f32), (1.9, 0.36), (0.85, 1.0)] {
        for i in 0..=n {
            let c = cols[i];
            let m = |j: usize| (c[j] * mult).clamp(0.0, 255.0) as u8;
            if m(3) < 3 {
                continue;
            }
            p.circle_filled(pts[i], rad, Color32::from_rgba_premultiplied(m(0), m(1), m(2), m(3)));
        }
    }
}

const BTN_HOVER: Color32 = Color32::from_rgb(31, 31, 40);

/// Жирный текст: в шрифте egui нет жирного начертания, поэтому буквы рисуются дважды с крошечным сдвигом.
fn bold_text(p: &egui::Painter, pos: egui::Pos2, align: Align2, text: &str, size: f32, color: Color32) {
    for dx in [-0.4_f32, 0.4] {
        p.text(pos + Vec2::new(dx, 0.0), align, text, FontId::proportional(size), color);
    }
}

/// Режим «всегда»: свет ходит по кнопке сам.
fn loop_ring(ui: &egui::Ui, rect: egui::Rect, radius: f32, k: f32) {
    let anim = ANIM_ON.load(Ordering::Relaxed);
    let now = if anim { ui.input(|i| i.time) as f32 } else { 0.0 };
    if anim {
        ui.ctx().request_repaint();
    }
    glow_ring(ui.painter(), rect.shrink(0.5), radius, ring_phase(now / 5.4), k);
}

fn icon_button(ui: &mut egui::Ui, icon: NavIcon, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    let resp = if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    let t = if enabled { hover_t(ui, &resp) } else { 0.0 };
    let pressed = enabled && resp.is_pointer_button_down_on();
    let rect = if pressed { rect.shrink(1.5) } else { rect };
    let color = if enabled { mix(Color32::from_rgb(190, 192, 206), Color32::WHITE, t) } else { Color32::from_rgb(92, 88, 126) };
    // Стрелка «обновить» делает оборот при каждом нажатии.
    let rot = if matches!(icon, NavIcon::Reload) {
        let id = egui::Id::new("reload_turns");
        let mut target: f32 = ui.ctx().data_mut(|d| d.get_temp(id).unwrap_or(0.0));
        if enabled && resp.clicked() {
            target += std::f32::consts::TAU;
            ui.ctx().data_mut(|d| d.insert_temp(id, target));
        }
        ui.ctx().avt(id.with("a"), target, 0.7)
    } else {
        0.0
    };
    let p = ui.painter();
    let rect = raised_base(p, rect, 12.0, t, 0.0, enabled);
    let c = rect.center();
    match icon {
        NavIcon::Back | NavIcon::Forward => {
            let d = if matches!(icon, NavIcon::Back) { -1.0 } else { 1.0 };
            // Шеврон «‹» / «›»: при наведении сдвигается в свою сторону.
            let c = c + Vec2::new(1.5 * d * t, 0.0);
            let st = Stroke::new(2.5_f32, color);
            let (x0, x1) = (-2.6 * d, 3.2 * d);
            let (a, tip, b) = (c + Vec2::new(x0, -6.2), c + Vec2::new(x1, 0.0), c + Vec2::new(x0, 6.2));
            p.add(egui::Shape::line(vec![a, tip, b], st));
            for q in [a, b] {
                p.circle_filled(q, st.width / 2.0, color);
            }
            p.circle_filled(tip, st.width / 2.0, color);
        }
        NavIcon::Reload => {
            let r = 6.4_f32;
            let st = Stroke::new(2.3_f32, color);
            let (a0, a1) = (-0.9_f32 + rot, -0.9_f32 + rot + std::f32::consts::TAU * 0.74);
            let pts: Vec<egui::Pos2> = (0..=28)
                .map(|i| {
                    let a = a0 + (a1 - a0) * (i as f32 / 28.0);
                    c + Vec2::new(r * a.cos(), r * a.sin())
                })
                .collect();
            let end = *pts.last().unwrap_or(&c);
            p.add(egui::Shape::line(pts, st));
            // Заполненный наконечник по касательной в конце дуги.
            let tangent = Vec2::new(-a1.sin(), a1.cos());
            let normal = Vec2::new(a1.cos(), a1.sin());
            p.add(egui::Shape::convex_polygon(
                vec![end + tangent * 4.6, end + normal * 4.2 - tangent * 0.8, end - normal * 4.2 - tangent * 0.8],
                color,
                Stroke::NONE,
            ));
        }
    }
    resp
}

/// Спокойная «стеклянная» строка внизу панели: цвет появляется только при наведении, открытом меню и активной загрузке.
fn downloads_button(ui: &mut egui::Ui, active: usize, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let t = hover_t(ui, &resp);
    let o = ui.ctx().abt(resp.id.with("open"), open, 0.25);
    let time = ui.input(|i| i.time) as f32;
    let busy = active > 0;
    let p = ui.painter();
    let lit_up = t.max(o);
    let rect = raised_base(p, rect, 13.0, lit_up, 0.0, true);
    let bob = if busy { ((time * 5.0).sin() * 0.5 + 0.5) * 3.5 } else { 1.5 * t };
    let ink = mix(MUTED, mix(TEXT, link(), o), lit_up.max(if busy { 1.0 } else { 0.0 }));
    let c = rect.left_center() + Vec2::new(24.0, -1.0);
    let st = Stroke::new(1.9_f32, ink);
    p.line_segment([c + Vec2::new(0.0, -7.0 + bob), c + Vec2::new(0.0, 3.0 + bob)], st);
    p.line_segment([c + Vec2::new(-4.5, -1.5 + bob), c + Vec2::new(0.0, 3.5 + bob)], st);
    p.line_segment([c + Vec2::new(4.5, -1.5 + bob), c + Vec2::new(0.0, 3.5 + bob)], st);
    p.line_segment([c + Vec2::new(-7.5, 7.5), c + Vec2::new(7.5, 7.5)], st);
    p.line_segment([c + Vec2::new(-7.5, 3.0), c + Vec2::new(-7.5, 7.5)], st);
    p.line_segment([c + Vec2::new(7.5, 3.0), c + Vec2::new(7.5, 7.5)], st);
    bold_text(p, rect.left_center() + Vec2::new(46.0, 0.0), Align2::LEFT_CENTER, "Загрузки", 14.5, mix(Color32::from_rgb(225, 226, 236), Color32::WHITE, lit_up));
    if busy {
        // Счётчик активных загрузок и бегущая полоска по нижней кромке.
        let b = rect.right_center() - Vec2::new(22.0, 0.0);
        grad_rrect(p, egui::Rect::from_center_size(b, Vec2::splat(22.0)), 11.0, accent_a(), accent_b(), false);
        p.text(b, Align2::CENTER_CENTER, active.to_string(), FontId::proportional(12.5), Color32::WHITE);
        let w = rect.width();
        let x = rect.left() + ((time * 0.7) % 1.0) * (w + 60.0) - 60.0;
        let seg = egui::Rect::from_min_max(egui::pos2(x.max(rect.left() + 8.0), rect.bottom() - 3.0), egui::pos2((x + 60.0).min(rect.right() - 8.0), rect.bottom() - 1.0));
        if seg.width() > 1.0 {
            grad_rrect(p, seg, 1.0, alpha(accent_a(), 0), accent_b(), false);
        }
        ui.ctx().request_repaint();
    }
    resp
}

/// Тонкая «призрачная» строка «Новая вкладка»: цвет появляется при наведении.
fn new_tab_row(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let t = hover_t(ui, &resp);
    let p = ui.painter();
    let rect = raised_base(p, rect, 12.0, t, 0.0, true);
    let c = rect.left_center() + Vec2::new(22.0, 0.0);
    let ink = mix(MUTED, Color32::WHITE, t);
    let st = Stroke::new(1.8_f32, ink);
    let spin = t * 0.6;
    let (sn, cs) = spin.sin_cos();
    for (dx, dy) in [(6.0_f32, 0.0_f32), (0.0, 6.0)] {
        let (a, b) = (Vec2::new(-dx, -dy), Vec2::new(dx, dy));
        let rot = |v: Vec2| Vec2::new(v.x * cs - v.y * sn, v.x * sn + v.y * cs);
        p.line_segment([c + rot(a), c + rot(b)], st);
    }
    bold_text(p, rect.left_center() + Vec2::new(42.0, 0.0), Align2::LEFT_CENTER, "Новая вкладка", 14.5, mix(Color32::from_rgb(225, 226, 236), Color32::WHITE, t));
    resp
}

/// Открывает файл программой по умолчанию (macOS и Linux нужны только для разработки).
fn open_path(path: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Показывает файл в папке (на Windows сразу выделяет его).
fn reveal_path(path: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    #[cfg(not(windows))]
    if let Some(dir) = Path::new(path).parent() {
        open_path(&dir.to_string_lossy());
    }
}

/// Кнопка меню (настройки, история, пароли): три ползунка, которые при наведении «переезжают».
fn dots_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let t = hover_t(ui, &resp);
    let o = ui.ctx().abt(resp.id.with("open"), open, 0.2);
    let rect = if resp.is_pointer_button_down_on() { rect.shrink(1.5) } else { rect };
    let p = ui.painter();
    let rect = raised_base(p, rect, 12.0, t, o, true);
    let ink = mix(mix(Color32::from_rgb(222, 218, 244), Color32::WHITE, t), Color32::WHITE, o);
    let c = rect.center();
    let st = Stroke::new(1.8_f32, ink);
    let (from, to) = ([-4.0_f32, 4.0, -1.0], [4.0_f32, -4.0, 4.5]);
    let k = (t * 0.7 + o * 0.3).clamp(0.0, 1.0);
    for (i, dy) in [-7.5_f32, 0.0, 7.5].into_iter().enumerate() {
        let x = from[i] + (to[i] - from[i]) * k;
        // Линия идёт по обе стороны от кружка, а не под ним.
        round_line(p, c + Vec2::new(-9.0, dy), c + Vec2::new(x - 3.8, dy), st);
        round_line(p, c + Vec2::new(x + 3.8, dy), c + Vec2::new(9.0, dy), st);
        p.circle_stroke(c + Vec2::new(x, dy), 2.7, st);
    }
    resp
}

fn menu_item(ui: &mut egui::Ui, text: &str, color: Color32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 42.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let t = hover_t(ui, &resp);
    let p = ui.painter();
    glass_base(p, rect, 13.0, t, 0.0);
    // Цветная «пуговка» с первой буквой.
    let tint = if color == TEXT { link() } else { color };
    let b = egui::Rect::from_center_size(rect.left_center() + Vec2::new(22.0, 0.0), Vec2::splat(28.0 + 2.0 * t));
    p.rect_filled(b, CornerRadius::same(9), Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), (38.0 + 40.0 * t) as u8));
    let letter: String = text.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    p.text(b.center(), Align2::CENTER_CENTER, letter, FontId::proportional(14.0), tint);
    bold_text(p, rect.left_center() + Vec2::new(44.0 + 3.0 * t, 0.0), Align2::LEFT_CENTER, text, 15.0, mix(color, Color32::WHITE, 0.35 * t));
    resp.clicked()
}

/// Менеджер паролей: пока не введён мастер-пароль, ключа нет и записи не расшифрованы.
#[derive(Default)]
struct VaultUi {
    exists: bool,
    key: Option<[u8; 32]>,
    salt: [u8; 16],
    entries: Vec<store::Entry>,
    pw: String,
    pw2: String,
    site: String,
    login: String,
    pass: String,
    shown: std::collections::HashSet<usize>,
    confirm_delete: Option<usize>,
    status: String,
}

fn pill_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(15.0), TEXT);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 28.0, 36.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let t = hover_t(ui, &resp);
    let p = ui.painter();
    glass_base(p, rect, 13.0, t, 0.0);
    bold_text(p, rect.center(), Align2::CENTER_CENTER, text, 15.0, Color32::WHITE);
    resp
}

/// Маленькая кнопка для тесных списков.
fn chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.0), TEXT);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 20.0, 28.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let t = hover_t(ui, &resp);
    let p = ui.painter();
    glass_base(p, rect, 10.0, t, 0.0);
    drop(galley);
    bold_text(p, rect.center(), Align2::CENTER_CENTER, text, 13.0, Color32::WHITE);
    resp
}

fn accent_paint(ui: &mut egui::Ui, rect: egui::Rect, resp: &egui::Response, text: &str) {
    let t = hover_t(ui, resp);
    let p = ui.painter();
    p.rect_filled(rect.translate(Vec2::new(0.0, 3.0)).shrink2(Vec2::new(6.0, 0.0)), CornerRadius::same(14), alpha(accent_b(), (18.0 + 46.0 * t) as u8));
    // Тёмная кнопка с лёгким отсветом темы изнутри; по ободку бежит свет.
    glass_base(p, rect, 14.0, t, 0.0);
    bold_text(p, rect.center(), Align2::CENTER_CENTER, text, 15.0, Color32::WHITE);
}

/// Градиентная кнопка по ширине текста.
fn accent_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let w = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(15.0), TEXT).size().x + 36.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 38.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    accent_paint(ui, rect, &resp, text);
    resp
}

/// То же на всю ширину колонки.
fn accent_wide(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    accent_paint(ui, rect, &resp, text);
    resp
}



/// Кнопка «Функции»: цветок-роза на градиентной плитке, при наведении приподнимается и поворачивается.
fn sticker_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect0, resp) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let t = hover_t(ui, &resp);
    let o = ui.ctx().abt(resp.id.with("open"), open, 0.25);
    let rect = rect0.translate(Vec2::new(0.0, -1.5 * t));
    let p = ui.painter();
    p.rect_filled(rect0.translate(Vec2::new(0.0, 3.0)).expand(1.0 + 1.5 * t), CornerRadius::same(15), alpha(accent_a(), (28.0 + 50.0 * t + 40.0 * o) as u8));
    grad_rrect(p, rect, 13.0, mix(accent_a(), lit(accent_a()), t), mix(accent_b(), lit(accent_b()), t), true);
    p.rect_stroke(rect, CornerRadius::same(13), Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(255, 255, 255, (30.0 + 170.0 * o) as u8)), egui::StrokeKind::Inside);
    let c = rect.center();
    let spin = t * 0.55 + o * 0.45;
    let ring = |n: usize, dist: f32, rad: f32, off: f32, col: Color32| {
        for k in 0..n {
            let a = off + spin + k as f32 * std::f32::consts::TAU / n as f32;
            p.circle_filled(c + Vec2::new(a.cos(), a.sin()) * dist, rad, col);
        }
    };
    ring(5, 6.6, 5.6, -1.57, Color32::from_rgba_unmultiplied(255, 255, 255, 235));
    ring(5, 3.2, 3.9, -1.57 + 0.63, Color32::from_rgb(255, 205, 225));
    p.circle_filled(c, 2.6, Color32::from_rgb(224, 64, 120));
    resp
}

// ───────────────────────── Звук в микрофон ─────────────────────────
//
// Windows не даёт программе создать микрофон, поэтому нужен виртуальный кабель, например бесплатный
// VB-CABLE: звуки играют в его «вход» (CABLE Input), а Discord или игра слушают его «выход»
// (CABLE Output) как обычный микрофон. Свой голос программа подмешивает в тот же кабель, иначе
// собеседники слышали бы только звуки.

const SETTINGS_FILE: &str = "helper_settings.txt";
const VB_CABLE_URL: &str = "https://vb-audio.com/Cable/";
/// Запас на ~0,2 с звука: если кабель отстаёт, лишнее отбрасывается, а не копится задержкой.
const MIC_BUFFER: usize = 19_200;

#[derive(Clone, PartialEq, Debug)]
struct AudioCfg {
    /// Играть звуки в микрофон (в виртуальный кабель).
    mic_on: bool,
    /// Устройство вывода, то есть виртуальный кабель.
    cable: String,
    /// Слышать звуки самому в наушниках.
    hear_self: bool,
    mic_volume: f32,
    /// Подмешивать свой голос в кабель.
    pass_on: bool,
    /// Настоящий микрофон.
    input: String,
    /// Громкость в наушниках.
    volume: f32,
    overlap: bool,
}

impl Default for AudioCfg {
    fn default() -> Self {
        AudioCfg {
            mic_on: false,
            cable: String::new(),
            hear_self: true,
            mic_volume: 1.0,
            pass_on: true,
            input: String::new(),
            volume: 1.0,
            overlap: false,
        }
    }
}

fn settings_to_text(c: &AudioCfg) -> String {
    format!(
        "mic_on={}\ncable={}\nhear_self={}\nmic_volume={}\npass_on={}\ninput={}\nvolume={}\noverlap={}\n",
        c.mic_on as u8, c.cable, c.hear_self as u8, c.mic_volume, c.pass_on as u8, c.input, c.volume, c.overlap as u8
    )
}

fn settings_from_text(s: &str) -> AudioCfg {
    let mut c = AudioCfg::default();
    let flag = |v: &str| v.trim() == "1";
    let vol = |v: &str, d: f32| v.trim().parse::<f32>().ok().filter(|x| x.is_finite()).map(|x| x.clamp(0.0, 1.5)).unwrap_or(d);
    for line in s.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "mic_on" => c.mic_on = flag(v),
            "cable" => c.cable = v.trim().to_string(),
            "hear_self" => c.hear_self = flag(v),
            "mic_volume" => c.mic_volume = vol(v, c.mic_volume),
            "pass_on" => c.pass_on = flag(v),
            "input" => c.input = v.trim().to_string(),
            "volume" => c.volume = vol(v, c.volume),
            "overlap" => c.overlap = flag(v),
            _ => {}
        }
    }
    c
}

fn load_settings() -> AudioCfg {
    std::fs::read_to_string(SETTINGS_FILE).map(|s| settings_from_text(&s)).unwrap_or_default()
}

fn save_settings(c: &AudioCfg) {
    let _ = std::fs::write(SETTINGS_FILE, settings_to_text(c));
}

/// Похоже ли устройство на виртуальный кабель.
fn is_cable_name(n: &str) -> bool {
    let l = n.to_lowercase();
    ["cable", "vb-audio", "voicemeeter", "virtual"].iter().any(|k| l.contains(k))
}

fn pick_cable(outputs: &[String]) -> Option<String> {
    outputs.iter().find(|n| is_cable_name(n)).cloned()
}

/// Настоящий микрофон: первый вход, который не кабель (иначе голос пошёл бы по кругу).
fn pick_input(inputs: &[String]) -> Option<String> {
    inputs.iter().find(|n| !is_cable_name(n)).cloned()
}

fn list_output_names() -> Vec<String> {
    rodio::speakers::available_outputs()
        .map(|v| v.iter().map(|o| o.to_string()).collect())
        .unwrap_or_default()
}

fn list_input_names() -> Vec<String> {
    rodio::microphone::available_inputs()
        .map(|v| v.iter().map(|i| i.to_string()).collect())
        .unwrap_or_default()
}

fn open_output_by_name(name: &str) -> Result<rodio::MixerDeviceSink, String> {
    let outputs = rodio::speakers::available_outputs().map_err(|e| e.to_string())?;
    let out = outputs
        .into_iter()
        .find(|o| o.to_string() == name)
        .ok_or_else(|| format!("устройство «{name}» не найдено"))?;
    rodio::speakers::SpeakersBuilder::new()
        .device(out)
        .map_err(|e| e.to_string())?
        .default_config()
        .map_err(|e| e.to_string())?
        .open_mixer()
        .map_err(|e| e.to_string())
}

fn open_decoder(path: &Path) -> Result<rodio::Decoder<std::io::BufReader<File>>, String> {
    let f = File::open(path).map_err(|e| e.to_string())?;
    rodio::Decoder::try_from(f).map_err(|e| e.to_string())
}

/// Голос с микрофона как источник звука для кабеля. Читает из очереди без ожидания: если
/// микрофон не успел, отдаёт тишину и не тормозит звуки в том же кабеле.
struct MicFeed {
    rx: mpsc::Receiver<f32>,
    channels: rodio::ChannelCount,
    rate: rodio::SampleRate,
    stop: Arc<AtomicBool>,
}

impl Iterator for MicFeed {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        if self.stop.load(Ordering::Relaxed) {
            return None;
        }
        match self.rx.try_recv() {
            Ok(s) => Some(s),
            Err(mpsc::TryRecvError::Empty) => Some(0.0),
            Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }
}

impl Source for MicFeed {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Запущенное подмешивание голоса. Пока значение живёт, микрофон слушается и идёт в кабель.
struct Passthrough {
    stop: Arc<AtomicBool>,
}

impl Drop for Passthrough {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn start_passthrough(input: &str, sink: &rodio::MixerDeviceSink) -> Result<Passthrough, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel::<f32>(MIC_BUFFER);
    type Ready = Result<(rodio::ChannelCount, rodio::SampleRate), String>;
    let (ready_tx, ready_rx) = mpsc::channel::<Ready>();

    // Микрофон открывается и читается в своём потоке: чтение блокирующее, а звуковой поток кабеля
    // ждать нельзя.
    let name = input.to_string();
    let stop_thread = stop.clone();
    std::thread::spawn(move || {
        let opened = (|| -> Result<rodio::microphone::Microphone, String> {
            let inputs = rodio::microphone::available_inputs().map_err(|e| e.to_string())?;
            let inp = inputs
                .into_iter()
                .find(|i| i.to_string() == name)
                .ok_or_else(|| format!("микрофон «{name}» не найден"))?;
            rodio::microphone::MicrophoneBuilder::new()
                .device(inp)
                .map_err(|e| e.to_string())?
                .default_config()
                .map_err(|e| e.to_string())?
                .open_stream()
                .map_err(|e| e.to_string())
        })();
        let mut mic = match opened {
            Ok(m) => m,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        let _ = ready_tx.send(Ok((mic.channels(), mic.sample_rate())));
        while !stop_thread.load(Ordering::Relaxed) {
            match mic.next() {
                Some(s) => {
                    let _ = tx.try_send(s);
                }
                None => break,
            }
        }
    });

    let (channels, rate) = ready_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| "микрофон не ответил".to_string())??;
    sink.mixer().add(MicFeed { rx, channels, rate, stop: stop.clone() });
    Ok(Passthrough { stop })
}

// ───────────────────────── Приложение ─────────────────────────

/// Куда «паркуем» страницу, которая должна играть в фоне, но не мешать.
const OFFSCREEN: [f32; 4] = [-12000.0, -12000.0, 640.0, 480.0];

/// Одна вкладка: веб-страница (WebView2) внутри окна. Создаётся при первом показе.
struct Pane {
    view: Option<web::Browser>,
    error: String,
    start_url: String,
    rect: Option<[f32; 4]>,
    title: Arc<Mutex<String>>,
    new_tx: mpsc::Sender<String>,
    new_rx: mpsc::Receiver<String>,
    /// Страница просит показать её на весь экран (видео на YouTube и т. п.).
    fs: Arc<AtomicBool>,
    /// Видео свёрнуто в маленькое окно.
    pip: Arc<AtomicBool>,
    /// Можно ли идти назад (бит 1) и вперёд (бит 2).
    nav: Arc<AtomicU8>,
    /// На странице играет звук.
    audio: Arc<AtomicBool>,
    /// Звук вкладки выключен пользователем.
    muted: bool,
    last_mute_push: Option<Instant>,
    /// Папка, в которой лежит вкладка.
    group: Option<u32>,
    /// Адрес страницы, спрошенный у браузера недавно (спрашивать каждый кадр дорого).
    url_cache: std::cell::RefCell<Option<(Instant, Option<String>)>>,
    /// Вкладка инкогнито: ничего не пишется ни в историю, ни в сессию.
    incognito: bool,
    /// Последний адрес, записанный в историю.
    logged_url: String,
    url_changed_at: Instant,
    /// Запись истории, у которой ещё нет названия страницы.
    hist_idx: Option<usize>,
}

impl Pane {
    fn new(url: &str) -> Self {
        let (new_tx, new_rx) = mpsc::channel();
        Pane {
            view: None,
            error: String::new(),
            start_url: url.to_string(),
            rect: None,
            title: Arc::new(Mutex::new(String::new())),
            new_tx,
            new_rx,
            fs: Arc::new(AtomicBool::new(false)),
            pip: Arc::new(AtomicBool::new(false)),
            nav: Arc::new(AtomicU8::new(0)),
            audio: Arc::new(AtomicBool::new(false)),
            muted: false,
            last_mute_push: None,
            group: None,
            url_cache: std::cell::RefCell::new(None),
            incognito: false,
            logged_url: String::new(),
            url_changed_at: Instant::now(),
            hist_idx: None,
        }
    }

    fn private(url: &str) -> Self {
        let mut p = Pane::new(url);
        p.incognito = true;
        p
    }

    fn title_text(&self) -> String {
        self.title.lock().map(|t| t.trim().to_string()).unwrap_or_default()
    }

    /// Показывает страницу ровно в `rect` (страница это отдельное окно поверх egui).
    fn show(&mut self, frame: &eframe::Frame, ui: &egui::Ui, rect: egui::Rect) {
        let ppp = ui.ctx().pixels_per_point();
        let bounds = [rect.min.x * ppp, rect.min.y * ppp, rect.width() * ppp, rect.height() * ppp];
        if self.view.is_none() && self.error.is_empty() {
            match web::Browser::new(frame, &self.start_url, bounds, self.title.clone(), self.new_tx.clone(), self.incognito, self.fs.clone(), self.pip.clone(), self.nav.clone(), self.audio.clone()) {
                Ok(b) => {
                    self.view = Some(b);
                    self.rect = Some(bounds);
                }
                Err(e) => self.error = e,
            }
        }
        if !self.error.is_empty() {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!(
                    "Не удалось открыть встроенную страницу:\n{}\n\nЕсли у вас Windows 10, установите «WebView2 Runtime» с сайта Microsoft.",
                    self.error
                ),
                FontId::proportional(15.0),
                MUTED,
            );
            return;
        }
        if let Some(b) = &self.view {
            if self.rect != Some(bounds) {
                b.set_bounds(bounds);
                self.rect = Some(bounds);
            }
        }
    }

    /// Прячет страницу за край окна, но не скрывает её: звук и вход продолжают работать.
    fn park(&mut self) {
        if let Some(b) = &self.view {
            if self.rect != Some(OFFSCREEN) {
                b.set_bounds(OFFSCREEN);
                self.rect = Some(OFFSCREEN);
            }
        }
    }

    fn url(&self) -> Option<String> {
        let view = self.view.as_ref()?;
        let mut cache = self.url_cache.borrow_mut();
        if let Some((at, url)) = cache.as_ref() {
            if at.elapsed() < Duration::from_millis(300) {
                return url.clone();
            }
        }
        let url = view.url();
        *cache = Some((Instant::now(), url.clone()));
        url
    }

    fn current_url(&self) -> String {
        self.url().filter(|u| !u.is_empty()).unwrap_or_else(|| self.start_url.clone())
    }

    fn label(&self) -> String {
        let title = self.title_text();
        let base = if title.is_empty() { tab_label("", &self.current_url()) } else { title };
        if self.incognito {
            format!("Инкогнито · {base}")
        } else {
            base
        }
    }
}

/// Один играющий звук: в наушниках и/или в кабеле (микрофоне).
struct Playing {
    rel: String,
    local: Option<rodio::Player>,
    mic: Option<rodio::Player>,
    total: Option<Duration>,
}

impl Playing {
    fn players(&self) -> impl Iterator<Item = &rodio::Player> {
        self.local.iter().chain(self.mic.iter())
    }
    fn stop(&self) {
        for p in self.players() {
            p.stop();
        }
    }
    fn empty(&self) -> bool {
        self.players().all(|p| p.empty())
    }
    fn pos(&self) -> Duration {
        self.players().next().map(|p| p.get_pos()).unwrap_or_default()
    }
}

fn start_player(
    mixer: &rodio::mixer::Mixer,
    path: &Path,
    volume: f32,
) -> Result<(rodio::Player, Option<Duration>), String> {
    let source = open_decoder(path)?;
    let total = source.total_duration();
    let player = rodio::Player::connect_new(mixer);
    player.set_volume(volume);
    player.append(source);
    Ok((player, total))
}

struct Spin {
    start: Instant,
    duration: f64,
    from: f64,
    delta: f64,
}

enum TileAction {
    Toggle,
    Rename,
    Hotkey,
    Move(String),
    Delete,
}

type AudioKey = (bool, String, bool, String);

/// Папка вкладок: её вкладки всегда идут в списке подряд.
#[derive(Clone)]
struct TabGroup {
    id: u32,
    name: String,
    color: usize,
    collapsed: bool,
}

const GROUP_COLORS: [Color32; 6] = [
    Color32::from_rgb(129, 140, 248),
    Color32::from_rgb(244, 114, 182),
    Color32::from_rgb(52, 211, 153),
    Color32::from_rgb(251, 191, 36),
    Color32::from_rgb(56, 189, 248),
    Color32::from_rgb(251, 146, 60),
];

enum TabAct {
    Mute(usize),
    NewGroup(usize),
    Join(usize, u32),
    Leave(usize),
    Toggle(u32),
    Rename(u32),
    RenameDone,
    Color(u32),
    Ungroup(u32),
    CloseGroup(u32),
}

struct App {
    tabs: Vec<Pane>,
    tab_groups: Vec<TabGroup>,
    upd: Option<store::UpdateInfo>,
    upd_rx: Option<mpsc::Receiver<Result<Option<store::UpdateInfo>, String>>>,
    upd_dl: Option<mpsc::Receiver<Result<std::path::PathBuf, String>>>,
    upd_hidden: bool,
    /// Проверку запросил сам пользователь: об итоге сообщаем даже когда обновлений нет.
    upd_manual: bool,
    upd_msg: String,
    next_gid: u32,
    /// Папка, название которой сейчас правится.
    grp_rename: Option<u32>,
    grp_edit: String,
    grp_focus: bool,
    last_groups: String,
    /// Закрытые вкладки инкогнито: страницу держим ещё секунду, пока стираются её данные.
    dying: Vec<(Instant, Pane)>,
    active: usize,
    addr: String,
    addr_focused: bool,
    tools_open: bool,
    /// Какие окна функций открыты (в порядке TOOLS).
    tool_win: [bool; 8],
    dots_open: bool,
    /// Сейчас окно на весь экран из-за страницы; адрес, с которого это началось.
    page_fs: bool,
    applied_zoom: u32,
    applied_search: String,
    applied_scale: u32,
    brand: &'static str,
    ext_input: String,
    ext_status: String,
    ext_list: Vec<(String, String)>,
    ext_rx: Option<mpsc::Receiver<Result<String, String>>>,
    menu_pos: egui::Pos2,
    dl_open: bool,
    dl_pos: egui::Pos2,
    dl_btn: egui::Rect,
    rnd_die: u8,
    rnd_coin: u8,
    rnd_pop: Instant,
    menu_btns: Vec<egui::Rect>,
    mini: Option<(Option<String>, egui::Vec2)>,
    fs_url: String,
    rng: Rng,

    bcfg: store::BrowserCfg,
    saved_bcfg: store::BrowserCfg,
    history: Vec<store::Visit>,
    history_dirty: bool,
    history_saved_at: Instant,
    hist_search: String,
    hist_confirm_clear: bool,
    last_session: String,
    persist_at: Instant,
    vault: VaultUi,
    settings_status: String,
    downloads: Vec<store::Download>,

    calc_input: String,
    calc_error: String,
    calc_history: Vec<String>,

    rnd_min: i64,
    rnd_max: i64,
    rnd_result: String,
    wheel_text: String,
    wheel_angle: f64,
    wheel_spin: Option<Spin>,
    wheel_result: String,
    wheel_history: Vec<String>,
    wheel_remove: bool,

    sounds: Vec<Sound>,
    groups: Vec<String>,
    hotkeys: HashMap<String, String>,
    sounds_loaded: bool,
    sound_status: String,
    sound_search: String,
    group_sel: Option<String>,
    new_group: Option<String>,
    /// (звук, новое имя, первый кадр)
    rename: Option<(String, String, bool)>,
    capture: Option<String>,
    confirm_delete: Option<String>,

    cfg: AudioCfg,
    saved_cfg: AudioCfg,
    applied: Option<AudioKey>,
    outputs: Vec<String>,
    inputs: Vec<String>,
    devices_loaded: bool,
    mic_status: String,
    audio: Option<rodio::MixerDeviceSink>,
    cable: Option<rodio::MixerDeviceSink>,
    pass: Option<Passthrough>,
    players: Vec<Playing>,
}

impl App {
    fn new() -> Self {
        let cfg = load_settings();
        let bcfg = store::load_cfg();
        let _ = FROZEN.set(bcfg.clone());
        sync_live(&bcfg);
        let (ia, ib) = store::theme_colors(&bcfg);
        ACC_A.store(ia, Ordering::Relaxed);
        ACC_B.store(ib, Ordering::Relaxed);
        ANIM_ON.store(bcfg.anim, Ordering::Relaxed);
        write_pages(&bcfg);
        if bcfg.clear_on_exit {
            // Браузер ещё не запущен, поэтому его папку с cookie и кэшем можно стереть целиком.
            let _ = std::fs::remove_dir_all("WebView2");
        }
        let (urls, active) = if bcfg.restore { store::load_session() } else { (Vec::new(), 0) };
        let (tabs, active) = if urls.is_empty() {
            (vec![Pane::new(&new_tab_url(&bcfg.home))], 0)
        } else {
            (urls.iter().map(|u| Pane::new(u)).collect(), active)
        };
        let mut tabs = tabs;
        let mut groups: Vec<TabGroup> = Vec::new();
        if bcfg.restore && !urls.is_empty() {
            for (gi, g) in store::load_groups(tabs.len()).into_iter().enumerate() {
                let id = gi as u32 + 1;
                for t in &mut tabs[g.start..g.start + g.len] {
                    t.group = Some(id);
                }
                groups.push(TabGroup { id, name: g.name, color: g.color % GROUP_COLORS.len(), collapsed: g.collapsed });
            }
        }
        let next_gid = groups.len() as u32 + 1;
        let mut app = App {
            tabs,
            tab_groups: groups,
            upd: None,
            upd_rx: None,
            upd_dl: None,
            upd_hidden: false,
            upd_manual: false,
            upd_msg: String::new(),
            next_gid,
            grp_rename: None,
            grp_edit: String::new(),
            grp_focus: false,
            last_groups: String::new(),
            dying: Vec::new(),
            active,
            addr: String::new(),
            addr_focused: false,
            tools_open: false,
            tool_win: [false; 8],
            dots_open: false,
            page_fs: false,
            applied_zoom: 100,
            applied_search: String::new(),
            applied_scale: 0,
            brand: if rand_below(10_000) == 0 { "Vpital" } else { "Petal" },
            ext_input: String::new(),
            ext_status: String::new(),
            ext_list: list_extensions(),
            ext_rx: None,
            menu_pos: egui::Pos2::ZERO,
            dl_open: false,
            dl_pos: egui::Pos2::ZERO,
            dl_btn: egui::Rect::NOTHING,
            rnd_die: 0,
            rnd_coin: 0,
            rnd_pop: Instant::now().checked_sub(Duration::from_secs(10)).unwrap_or_else(Instant::now),
            menu_btns: Vec::new(),
            mini: None,
            fs_url: String::new(),
            rng: Rng::new(),

            saved_bcfg: bcfg.clone(),
            bcfg,
            history: store::load_history(),
            history_dirty: false,
            history_saved_at: Instant::now(),
            hist_search: String::new(),
            hist_confirm_clear: false,
            last_session: String::new(),
            persist_at: Instant::now(),
            vault: VaultUi { exists: std::path::Path::new(store::VAULT_FILE).exists(), ..Default::default() },
            settings_status: String::new(),
            downloads: store::load_downloads(),

            calc_input: String::new(),
            calc_error: String::new(),
            calc_history: Vec::new(),

            rnd_min: 1,
            rnd_max: 100,
            rnd_result: String::new(),
            wheel_text: std::fs::read_to_string(WHEEL_FILE)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| WHEEL_DEFAULT.to_string()),
            wheel_angle: 0.0,
            wheel_spin: None,
            wheel_result: String::new(),
            wheel_history: Vec::new(),
            wheel_remove: false,

            sounds: Vec::new(),
            groups: Vec::new(),
            hotkeys: HashMap::new(),
            sounds_loaded: false,
            sound_status: String::new(),
            sound_search: String::new(),
            group_sel: None,
            new_group: None,
            rename: None,
            capture: None,
            confirm_delete: None,

            saved_cfg: cfg.clone(),
            cfg,
            applied: None,
            outputs: Vec::new(),
            inputs: Vec::new(),
            devices_loaded: false,
            mic_status: String::new(),
            audio: None,
            cable: None,
            pass: None,
            players: Vec::new(),
        };
        if app.bcfg.auto_update {
            app.start_update_check(false);
        }
        // Клавиши звуков должны работать и при закрытой панели.
        app.sounds_reload();
        if app.cfg.mic_on {
            app.devices_refresh();
        }
        app
    }

    // ── вкладки ──

    fn open_tab(&mut self, url: &str) {
        self.tabs.push(Pane::new(url));
        self.select_tab(self.tabs.len() - 1);
    }

    fn select_tab(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.active = i;
            self.addr = shown_url(&self.tabs[i].current_url());
            self.addr_focused = false;
        }
    }

    /// Переносит вкладку `from` так, чтобы она оказалась на месте `to` (индекс после удаления), и сохраняет выбор.
    fn move_tab(&mut self, from: usize, to: usize) {
        let to = to.min(self.tabs.len() - 1);
        if from == to {
            return;
        }
        let t = self.tabs.remove(from);
        self.tabs.insert(to, t);
        self.active = if self.active == from {
            to
        } else {
            let mut a = self.active;
            if from < a {
                a -= 1;
            }
            if to <= a {
                a += 1;
            }
            a
        };
    }

    fn group_span(&self, g: u32) -> Option<(usize, usize)> {
        let first = self.tabs.iter().position(|t| t.group == Some(g))?;
        let last = self.tabs.iter().rposition(|t| t.group == Some(g))?;
        Some((first, last))
    }

    fn set_group(&mut self, i: usize, g: Option<u32>) {
        if i >= self.tabs.len() || self.tabs[i].group == g {
            return;
        }
        let span = match g {
            Some(g) => self.group_span(g),
            None => self.tabs[i].group.and_then(|o| self.group_span(o)),
        };
        let to = span.map(|(_, last)| match g {
            // В папку: сразу после её последней вкладки.
            Some(_) => if i < last { last } else { last + 1 },
            // Из папки: сразу после папки.
            None => last,
        });
        self.tabs[i].group = g;
        if let Some(to) = to {
            self.move_tab(i, to);
        }
    }

    /// Закрытую вкладку инкогнито стираем вместе с данными сайтов, если других вкладок инкогнито не осталось.
    fn retire_private(&mut self, mut p: Pane) {
        if self.tabs.iter().any(|t| t.incognito) {
            return;
        }
        if let Some(v) = &p.view {
            v.navigate("about:blank");
            v.clear_data();
        }
        p.park();
        self.dying.push((Instant::now(), p));
    }

    /// Раз в кадр: глушит вкладки, которые пользователь выключил, и убирает отыгравшие страницы.
    fn pane_tick(&mut self, ctx: &egui::Context) {
        self.dying.retain(|(t, _)| t.elapsed() < Duration::from_millis(2000));
        let mut watch = !self.dying.is_empty();
        for t in &mut self.tabs {
            if t.muted {
                // Страница после перехода забывает о запрете, поэтому напоминаем.
                if t.last_mute_push.is_none_or(|a| a.elapsed() >= Duration::from_millis(450)) {
                    if let Some(v) = &t.view {
                        v.eval("window.__petalMute&&window.__petalMute(true)");
                    }
                    t.last_mute_push = Some(Instant::now());
                }
                watch = true;
            } else if t.audio.load(Ordering::Relaxed) {
                watch = true;
            }
        }
        if watch {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    fn toggle_mute(&mut self, i: usize) {
        if let Some(t) = self.tabs.get_mut(i) {
            t.muted = !t.muted;
            t.last_mute_push = Some(Instant::now());
            if let Some(v) = &t.view {
                v.eval(if t.muted { "window.__petalMute&&window.__petalMute(true)" } else { "window.__petalMute&&window.__petalMute(false)" });
            }
        }
    }

    fn prune_groups(&mut self) {
        let tabs = &self.tabs;
        self.tab_groups.retain(|g| tabs.iter().any(|t| t.group == Some(g.id)));
        if self.grp_rename.is_some_and(|r| !self.tab_groups.iter().any(|g| g.id == r)) {
            self.grp_rename = None;
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        let gone = self.tabs.remove(i);
        if gone.incognito && self.bcfg.wipe_incognito {
            self.retire_private(gone);
        }
        self.prune_groups();
        if self.tabs.is_empty() {
            self.tabs.push(Pane::new(&new_tab_url(&self.bcfg.home)));
        }
        if i < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        self.select_tab(self.active);
    }

    /// Ссылки «открыть в новом окне» с любой вкладки становятся вкладками.
    fn drain_new_tabs(&mut self) {
        let urls: Vec<String> = self.tabs.iter().flat_map(|t| t.new_rx.try_iter().collect::<Vec<_>>()).collect();
        for u in urls {
            if !u.trim().is_empty() {
                self.open_tab(&u);
            }
        }
    }

    fn go(&mut self, input: &str) {
        let url = normalize_url(input);
        let tab = &mut self.tabs[self.active];
        if let Some(b) = &tab.view {
            b.navigate(&url);
        } else {
            tab.start_url = url.clone();
        }
        self.addr = shown_url(&url);
    }

    // ── каркас окна ──

    /// Стрелки, кнопки меню и адресная строка: всё в левой колонке.
    fn nav_block(&mut self, ui: &mut egui::Ui) {
        let mut go: Option<String> = None;
        let mut toggle = false;
        let mut toggle_dots = false;
        let has = self.tabs[self.active].view.is_some();
        let nav = self.tabs[self.active].nav.load(Ordering::Relaxed);
        let mut btns: Vec<egui::Rect> = Vec::new();
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let tab = &self.tabs[self.active];
            if icon_button(ui, NavIcon::Back, has && nav & 1 != 0).clicked() && nav & 1 != 0 {
                if let Some(b) = &tab.view {
                    b.back();
                }
            }
            if icon_button(ui, NavIcon::Forward, has && nav & 2 != 0).clicked() && nav & 2 != 0 {
                if let Some(b) = &tab.view {
                    b.forward();
                }
            }
            if icon_button(ui, NavIcon::Reload, has).clicked() {
                if let Some(b) = &tab.view {
                    b.reload();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r1 = sticker_button(ui, self.tools_open);
                btns.push(r1.rect);
                if r1.clicked() {
                    toggle = true;
                }
                let r2 = dots_button(ui, self.dots_open);
                btns.push(r2.rect);
                if r2.clicked() {
                    toggle_dots = true;
                }
            });
        });
        self.menu_pos = row.response.rect.left_bottom() + Vec2::new(0.0, 8.0);
        self.menu_btns = btns;
        ui.add_space(8.0);
        let glow = ui.ctx().abt(egui::Id::new("addr_glow"), self.addr_focused, 0.2);
        let field = Frame::new()
            .fill(mix(Color32::from_rgba_unmultiplied(0, 0, 0, 110), Color32::from_rgba_unmultiplied(0, 0, 0, 150), glow))
            .corner_radius(CornerRadius::same(20))
            .stroke(Stroke::new(1.2_f32, mix(Color32::from_rgba_unmultiplied(255, 255, 255, 26), accent_b(), glow)))
            .inner_margin(Margin { left: 38, right: 14, top: 7, bottom: 7 })
            .show(ui, |ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.addr)
                    .frame(false)
                    .desired_width(ui.available_width())
                    .hint_text(RichText::new("Адрес или поиск").color(Color32::from_rgb(130, 132, 150)))
                    .font(FontId::proportional(15.0)),
            );
            self.addr_focused = resp.has_focus();
            if resp.gained_focus() {
                if let Some(b) = &self.tabs[self.active].view {
                    b.focus_parent();
                }
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                go = Some(self.addr.clone());
            }
        });
        // Лупа слева в строке: при фокусе подсвечивается цветом темы.
        {
            let c = field.response.rect.left_center() + Vec2::new(19.0, -1.0);
            let st = Stroke::new(1.7_f32, mix(Color32::from_rgb(130, 132, 150), link(), glow));
            let p = ui.painter();
            p.circle_stroke(c, 5.2, st);
            p.line_segment([c + Vec2::new(3.8, 3.8), c + Vec2::new(8.0, 8.0)], st);
        }
        if let Some(g) = go {
            self.go(&g);
        }
        if toggle {
            self.tools_open = !self.tools_open;
            self.dots_open = false;
            self.dl_open = false;
        }
        if toggle_dots {
            self.dots_open = !self.dots_open;
            self.tools_open = false;
            self.dl_open = false;
        }
    }

    fn tabs_panel(&mut self, ctx: &egui::Context) {
        let mut select: Option<usize> = None;
        let mut close: Option<usize> = None;
        let mut new_tab = false;
        let mut tab_act: Option<TabAct> = None;
        let mut open_downloads = false;
        let active_downloads = self.downloads.iter().filter(|d| d.state == store::DL_ACTIVE).count();
        egui::SidePanel::left("tabs")
            .exact_width(self.bcfg.sidebar_w as f32)
            .resizable(false)
            .frame(Frame::new().fill(SIDE_BG).inner_margin(Margin::same(10)))
            .show(ctx, |ui| {
                let (g0, g1) = if self.tabs[self.active].incognito { (Color32::from_rgb(40, 40, 43), Color32::from_rgb(20, 20, 22)) } else { (mix(Color32::from_rgb(15, 15, 20), accent_b(), 0.16), Color32::from_rgb(15, 15, 20)) };
                let full = ui.max_rect().expand(10.0);
                grad_rrect(ui.painter(), full, 0.0, g0, g1, true);
                grad_rrect(ui.painter(), egui::Rect::from_min_max(egui::pos2(full.right() - 1.0, full.top()), full.right_bottom()), 0.0, alpha(accent_b(), 10), alpha(accent_b(), 70), true);
                if self.bcfg.glow && !self.tabs[self.active].incognito {
                    // Два мягких пятна акцентных цветов медленно плавают за панелью.
                    let t = if self.bcfg.anim { ui.input(|i| i.time) as f32 } else { 0.0 };
                    let w = full.width();
                    let c1 = full.left_top() + Vec2::new(w * 0.3 + (t * 0.3).sin() * 38.0, 130.0 + (t * 0.25).cos() * 46.0);
                    let c2 = full.left_bottom() + Vec2::new(w * 0.8 + (t * 0.21).cos() * 30.0, -170.0 + (t * 0.27).sin() * 55.0);
                    radial_glow(ui.painter(), c1, 200.0, alpha(accent_a(), 46));
                    radial_glow(ui.painter(), c2, 230.0, alpha(accent_b(), 42));
                    if self.bcfg.anim {
                        ui.ctx().request_repaint_after(Duration::from_millis(60));
                    }
                }
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    paint_logo(ui.painter(), r);
                    ui.label(RichText::new(self.brand).size(22.0).strong().color(TEXT));
                });
                ui.add_space(12.0);
                self.nav_block(ui);
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("ВКЛАДКИ").size(11.5).color(Color32::from_rgb(130, 132, 150)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(self.tabs.len().to_string()).size(11.5).color(Color32::from_rgb(130, 132, 150)));
                    });
                });
                ui.add_space(6.0);
                if new_tab_row(ui).clicked() {
                    new_tab = true;
                }
                ui.add_space(8.0);
                egui::TopBottomPanel::bottom("tabs_bottom").frame(Frame::NONE).show_inside(ui, |ui| {
                    ui.add_space(4.0);
                    let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                    grad_rrect(ui.painter(), line, 0.0, alpha(Color32::WHITE, 0), alpha(Color32::WHITE, 26), false);
                    ui.add_space(8.0);
                    let r = downloads_button(ui, active_downloads, self.dl_open);
                    self.dl_pos = r.rect.left_top() - Vec2::new(0.0, 8.0);
                    self.dl_btn = r.rect;
                    if r.clicked() {
                        open_downloads = true;
                    }
                });
                // 0 — обновить, 1 — позже, 2 — пропустить эту версию.
                let mut upd_act: Option<u8> = None;
                if let Some(u) = self.upd.as_ref().filter(|u| !self.upd_hidden && u.version != self.bcfg.skip_version) {
                    ui.add_space(8.0);
                    let busy = self.upd_dl.is_some();
                    Frame::new()
                        .fill(alpha(accent_b(), 34))
                        .stroke(Stroke::new(1.0_f32, alpha(accent_b(), 110)))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(format!("Вышла версия {}", u.version)).size(14.0).strong().color(Color32::WHITE));
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if ui.add_enabled(!busy, egui::Button::new(if busy { "Скачиваю…" } else { "Обновить" })).clicked() {
                                    upd_act = Some(0);
                                }
                                for (n, t) in [(1u8, "Позже"), (2u8, "Пропустить")] {
                                    if ui.add(egui::Label::new(RichText::new(t).size(12.5).color(MUTED)).sense(Sense::click())).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                                        upd_act = Some(n);
                                    }
                                }
                            });
                        });
                    ui.add_space(4.0);
                }
                match upd_act {
                    Some(0) => self.start_update(),
                    Some(1) => self.upd_hidden = true,
                    Some(_) => {
                        if let Some(u) = &self.upd {
                            self.bcfg.skip_version = u.version.clone();
                        }
                    }
                    None => {}
                }
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (i, tab) in self.tabs.iter().enumerate() {
                        let active = i == self.active;
                        let grp = tab.group.and_then(|g| self.tab_groups.iter().find(|x| x.id == g));
                        if let Some(g) = grp {
                            if i == 0 || self.tabs[i - 1].group != tab.group {
                                let count = self.tabs.iter().filter(|t| t.group == tab.group).count();
                                let col = GROUP_COLORS[g.color % GROUP_COLORS.len()];
                                let (hrow, hresp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
                                let ht = ui.ctx().abt(hresp.id.with("hov"), hresp.hovered(), 0.14);
                                let p = ui.painter();
                                p.rect_filled(hrow, CornerRadius::same(10), alpha(col, (26.0 + 26.0 * ht) as u8));
                                p.circle_filled(hrow.left_center() + Vec2::new(16.0, 0.0), 5.0, col);
                                if self.grp_rename == Some(g.id) {
                                    let er = egui::Rect::from_min_max(hrow.left_top() + Vec2::new(28.0, 3.0), hrow.right_bottom() - Vec2::new(30.0, 3.0));
                                    let te = ui.put(er, egui::TextEdit::singleline(&mut self.grp_edit).frame(false).desired_width(er.width()).font(FontId::proportional(14.0)));
                                    if self.grp_focus {
                                        te.request_focus();
                                        self.grp_focus = false;
                                    }
                                    if te.lost_focus() {
                                        tab_act = Some(TabAct::RenameDone);
                                    }
                                } else {
                                    let name = if g.name.trim().is_empty() { "Папка" } else { g.name.as_str() };
                                    let tr = egui::Rect::from_min_max(hrow.left_top() + Vec2::new(28.0, 0.0), hrow.right_bottom() - Vec2::new(40.0, 0.0));
                                    bold_text(&p.with_clip_rect(tr), hrow.left_center() + Vec2::new(28.0, 0.0), Align2::LEFT_CENTER, name, 14.0, mix(col, Color32::WHITE, 0.45));
                                }
                                // Число вкладок и стрелка: вправо, если папка свёрнута, вниз, если раскрыта.
                                let p = ui.painter();
                                p.text(hrow.right_center() - Vec2::new(26.0, 0.0), Align2::RIGHT_CENTER, count.to_string(), FontId::proportional(12.0), MUTED);
                                let ac = hrow.right_center() - Vec2::new(14.0, 0.0);
                                let pts = if g.collapsed {
                                    vec![ac + Vec2::new(-2.0, -4.0), ac + Vec2::new(2.0, 0.0), ac + Vec2::new(-2.0, 4.0)]
                                } else {
                                    vec![ac + Vec2::new(-4.0, -2.0), ac + Vec2::new(0.0, 2.0), ac + Vec2::new(4.0, -2.0)]
                                };
                                p.add(egui::Shape::line(pts, Stroke::new(1.6_f32, MUTED)));
                                if hresp.clicked() && self.grp_rename != Some(g.id) {
                                    tab_act = Some(TabAct::Toggle(g.id));
                                }
                                hresp.context_menu(|ui| {
                                    if ui.button("Переименовать").clicked() {
                                        tab_act = Some(TabAct::Rename(g.id));
                                        ui.close_menu();
                                    }
                                    if ui.button("Сменить цвет").clicked() {
                                        tab_act = Some(TabAct::Color(g.id));
                                        ui.close_menu();
                                    }
                                    if ui.button("Разгруппировать").clicked() {
                                        tab_act = Some(TabAct::Ungroup(g.id));
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    if ui.button("Закрыть все вкладки папки").clicked() {
                                        tab_act = Some(TabAct::CloseGroup(g.id));
                                        ui.close_menu();
                                    }
                                });
                            }
                            // Свёрнутая папка прячет вкладки, кроме открытой сейчас.
                            if g.collapsed && !active {
                                continue;
                            }
                        }
                        let (row_full, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), if self.bcfg.compact_tabs { 34.0 } else { 40.0 }), Sense::click());
                        // Вкладки папки сдвинуты вправо и помечены цветной чертой.
                        let row = if let Some(g) = grp {
                            let col = GROUP_COLORS[g.color % GROUP_COLORS.len()];
                            let bar = egui::Rect::from_min_max(row_full.left_top() + Vec2::new(5.0, 4.0), egui::pos2(row_full.left() + 8.0, row_full.bottom() - 4.0));
                            ui.painter().rect_filled(bar, CornerRadius::same(2), alpha(col, 200));
                            egui::Rect::from_min_max(row_full.left_top() + Vec2::new(12.0, 0.0), row_full.right_bottom())
                        } else {
                            row_full
                        };
                        let close_rect = egui::Rect::from_center_size(row.right_center() - Vec2::new(16.0, 0.0), Vec2::splat(22.0));
                        let sound_on = tab.audio.load(Ordering::Relaxed) || tab.muted;
                        let sound_rect = egui::Rect::from_center_size(close_rect.left_center() - Vec2::new(13.0, 0.0), Vec2::new(24.0, 24.0));
                        let sound_resp = if sound_on { Some(ui.interact(sound_rect, ui.id().with(("snd_tab", i)), Sense::click())) } else { None };
                        let close_resp = ui.interact(close_rect, ui.id().with(("close_tab", i)), Sense::click());
                        let ht = ui.ctx().abt(resp.id.with("hov"), resp.hovered(), 0.14);
                        let at = ui.ctx().abt(resp.id.with("act"), active, 0.22);
                        let p = ui.painter();
                        if ht > 0.01 {
                            p.rect_filled(row, CornerRadius::same(12), Color32::from_rgba_unmultiplied(255, 255, 255, (13.0 * ht * (1.0 - at)) as u8));
                        }
                        if at > 0.01 {
                            // Активная вкладка: стеклянная карточка с мягким градиентом темы и тонкой обводкой.
                            grad_rrect(p, row, 12.0, alpha(accent_a(), (52.0 * at) as u8), alpha(accent_b(), (60.0 * at) as u8), false);
                            p.rect_stroke(row, CornerRadius::same(12), Stroke::new(1.0_f32, alpha(accent_b(), (120.0 * at) as u8)), egui::StrokeKind::Inside);
                            loop_ring(ui, row, 12.0, 0.5 * at);
                        }
                        // Цветной кружок с первой буквой сайта.
                        let label = tab.label();
                        let hue = tab.current_url().bytes().filter(|b| *b != b'/').take(24).fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
                        let pal = [accent_a(), accent_b(), Color32::from_rgb(56, 189, 248), Color32::from_rgb(52, 211, 153), Color32::from_rgb(251, 146, 60), Color32::from_rgb(250, 204, 21)];
                        let avatar = if tab.incognito { PURPLE } else { pal[(hue % 6) as usize] };
                        let ac = row.left_center() + Vec2::new(22.0, 0.0);
                        let cur = tab.current_url();
                        let icon = if tab.incognito { None } else { favicon(ui.ctx(), &cur) };
                        if cur.starts_with("file://") && cur.ends_with("/petal_newtab.html") {
                            paint_logo(p, egui::Rect::from_center_size(ac, Vec2::splat(22.0)));
                        } else if let Some(tex) = icon {
                            p.circle_filled(ac, 12.0, Color32::from_rgb(236, 236, 244));
                            p.image(tex.id, egui::Rect::from_center_size(ac, Vec2::splat(16.0)), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        } else {
                            p.circle_filled(ac, 11.0, Color32::from_rgba_unmultiplied(avatar.r(), avatar.g(), avatar.b(), 60));
                            let initial: String = label.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "·".to_string());
                            p.text(ac, Align2::CENTER_CENTER, initial, FontId::proportional(13.0), avatar);
                        }
                        let text_rect = egui::Rect::from_min_max(
                            row.left_top() + Vec2::new(40.0, 0.0),
                            egui::pos2(if sound_on { sound_rect.left() - 1.0 } else { close_rect.left() - 2.0 }, row.bottom()),
                        );
                        p.with_clip_rect(text_rect).text(
                            row.left_center() + Vec2::new(40.0, 0.0),
                            Align2::LEFT_CENTER,
                            label,
                            FontId::proportional(14.0),
                            if active { Color32::WHITE } else { Color32::from_rgb(208, 210, 222) },
                        );
                        // Крестик виден у активной вкладки и при наведении.
                        let show_x = ht.max(at);
                        if close_resp.hovered() {
                            p.circle_filled(close_rect.center(), 10.0, Color32::from_rgba_unmultiplied(255, 255, 255, 30));
                        }
                        p.text(
                            close_rect.center(),
                            Align2::CENTER_CENTER,
                            "×",
                            FontId::proportional(17.0),
                            alpha(if close_resp.hovered() { TEXT } else { MUTED }, (255.0 * show_x.max(if close_resp.hovered() { 1.0 } else { 0.0 })) as u8),
                        );
                        if let Some(sr) = &sound_resp {
                            if sr.hovered() {
                                p.circle_filled(sound_rect.center(), 11.0, Color32::from_rgba_unmultiplied(255, 255, 255, 30));
                            }
                            speaker_icon(p, sound_rect.center(), if tab.muted { MUTED } else { mix(accent_b(), Color32::WHITE, 0.55) }, tab.muted);
                            sr.clone().on_hover_text(if tab.muted { "Включить звук вкладки" } else { "Выключить звук вкладки" }).on_hover_cursor(CursorIcon::PointingHand);
                        }
                        let sound_clicked = sound_resp.as_ref().is_some_and(|r| r.clicked());
                        resp.context_menu(|ui| {
                            if tab.audio.load(Ordering::Relaxed) || tab.muted {
                                if ui.button(if tab.muted { "Включить звук" } else { "Выключить звук" }).clicked() {
                                    tab_act = Some(TabAct::Mute(i));
                                    ui.close_menu();
                                }
                                ui.separator();
                            }
                            if ui.button("В новую папку").clicked() {
                                tab_act = Some(TabAct::NewGroup(i));
                                ui.close_menu();
                            }
                            if !self.tab_groups.is_empty() {
                                ui.menu_button("В папку", |ui| {
                                    for g in &self.tab_groups {
                                        if tab.group != Some(g.id) {
                                            let nm = if g.name.trim().is_empty() { "Папка" } else { g.name.as_str() };
                                            if ui.button(nm).clicked() {
                                                tab_act = Some(TabAct::Join(i, g.id));
                                                ui.close_menu();
                                            }
                                        }
                                    }
                                });
                            }
                            if tab.group.is_some() && ui.button("Убрать из папки").clicked() {
                                tab_act = Some(TabAct::Leave(i));
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Закрыть вкладку").clicked() {
                                close = Some(i);
                                ui.close_menu();
                            }
                        });
                        if sound_clicked {
                            tab_act = Some(TabAct::Mute(i));
                        } else if close_resp.clicked() || resp.middle_clicked() {
                            close = Some(i);
                        } else if resp.clicked() {
                            select = Some(i);
                        }
                        resp.on_hover_cursor(CursorIcon::PointingHand);
                    }
                });
            });
        if open_downloads {
            self.dl_open = !self.dl_open;
            self.tools_open = false;
            self.dots_open = false;
        }
        if let Some(a) = tab_act {
            self.apply_tab_act(a);
        }
        if new_tab {
            self.open_tab(&new_tab_url(&self.bcfg.home));
        } else if let Some(i) = close {
            self.close_tab(i);
        } else if let Some(i) = select {
            self.select_tab(i);
        }
    }

    fn start_update_check(&mut self, manual: bool) {
        if self.upd_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.upd_rx = Some(rx);
        self.upd_manual = manual;
        if manual {
            self.upd_msg = "Проверяю…".to_string();
            self.upd_hidden = false;
        }
        std::thread::spawn(move || {
            let _ = tx.send(check_update());
        });
    }

    fn is_portable() -> bool {
        std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("portable.txt").exists())).unwrap_or(false)
    }

    /// «Обновить»: установленную версию заменяет установщик, переносную пользователь скачивает со страницы релиза.
    fn start_update(&mut self) {
        let Some(info) = self.upd.clone() else { return };
        if Self::is_portable() || info.installer.is_empty() || !cfg!(windows) {
            self.open_tab(&info.page);
            self.upd_hidden = true;
            return;
        }
        if self.upd_dl.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.upd_dl = Some(rx);
        self.upd_msg = "Скачиваю обновление…".to_string();
        std::thread::spawn(move || {
            let _ = tx.send(download_installer(&info));
        });
    }

    fn poll_updates(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.upd_rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.upd_rx = None;
                    match r {
                        Ok(Some(info)) => {
                            self.upd_msg = format!("Доступна версия {}.", info.version);
                            self.upd = Some(info);
                        }
                        Ok(None) => {
                            if self.upd_manual {
                                self.upd_msg = format!("У вас последняя версия ({APP_VERSION}).");
                            }
                        }
                        Err(e) => {
                            if self.upd_manual {
                                self.upd_msg = e;
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(300)),
                Err(_) => self.upd_rx = None,
            }
        }
        if let Some(rx) = &self.upd_dl {
            match rx.try_recv() {
                Ok(Ok(path)) => {
                    self.upd_dl = None;
                    self.flush(true);
                    let started = std::process::Command::new(&path).args(["/SILENT", "/NORESTART", "/CLOSEAPPLICATIONS"]).spawn().is_ok();
                    if started {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        self.upd_msg = "Не удалось запустить установщик.".to_string();
                    }
                }
                Ok(Err(e)) => {
                    self.upd_dl = None;
                    self.upd_msg = e;
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(300)),
                Err(_) => self.upd_dl = None,
            }
        }
    }

    fn apply_tab_act(&mut self, a: TabAct) {
        match a {
            TabAct::Mute(i) => self.toggle_mute(i),
            TabAct::NewGroup(i) => {
                if i < self.tabs.len() && !self.tabs[i].incognito {
                    let id = self.next_gid;
                    self.next_gid += 1;
                    let color = (id as usize).wrapping_sub(1) % GROUP_COLORS.len();
                    self.tab_groups.push(TabGroup { id, name: String::new(), color, collapsed: false });
                    self.set_group(i, Some(id));
                    self.prune_groups();
                    self.grp_rename = Some(id);
                    self.grp_edit.clear();
                    self.grp_focus = true;
                }
            }
            TabAct::Join(i, g) => {
                if self.tabs.get(i).is_some_and(|t| !t.incognito) {
                    self.set_group(i, Some(g));
                    self.prune_groups();
                }
            }
            TabAct::Leave(i) => {
                self.set_group(i, None);
                self.prune_groups();
            }
            TabAct::Toggle(g) => {
                if let Some(x) = self.tab_groups.iter_mut().find(|x| x.id == g) {
                    x.collapsed = !x.collapsed;
                }
            }
            TabAct::Rename(g) => {
                if let Some(x) = self.tab_groups.iter().find(|x| x.id == g) {
                    self.grp_edit = x.name.clone();
                    self.grp_rename = Some(g);
                    self.grp_focus = true;
                }
            }
            TabAct::RenameDone => {
                if let Some(g) = self.grp_rename.take() {
                    let name: String = self.grp_edit.trim().chars().take(40).collect();
                    if let Some(x) = self.tab_groups.iter_mut().find(|x| x.id == g) {
                        x.name = name;
                    }
                }
            }
            TabAct::Color(g) => {
                if let Some(x) = self.tab_groups.iter_mut().find(|x| x.id == g) {
                    x.color = (x.color + 1) % GROUP_COLORS.len();
                }
            }
            TabAct::Ungroup(g) => {
                for t in &mut self.tabs {
                    if t.group == Some(g) {
                        t.group = None;
                    }
                }
                self.prune_groups();
            }
            TabAct::CloseGroup(g) => {
                while let Some(i) = self.tabs.iter().rposition(|t| t.group == Some(g)) {
                    self.close_tab(i);
                }
            }
        }
    }

    /// Маленькое меню по значку-стикеру: кнопка на каждую функцию.
    fn tools_menu(&mut self, ctx: &egui::Context) -> Option<egui::Rect> {
        let mut picked: Option<usize> = None;
        let wins = self.tool_win;
        let card = popup_card(ctx, "tools", self.tools_open, self.menu_pos, self.bcfg.sidebar_w as f32 - 20.0, false, |ui| {
            ui.label(RichText::new("Функции").size(13.0).color(MUTED));
            ui.add_space(2.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            for (i, (tool, name, color)) in TOOLS.iter().enumerate() {
                if in_dots(*tool) || matches!(*tool, Tool::Downloads | Tool::Sounds) {
                    continue;
                }
                if menu_item(ui, name, if wins[i] { *color } else { TEXT }) {
                    picked = Some(i);
                }
            }
        });
        if let Some(i) = picked {
            self.tool_win[i] = true;
            self.tools_open = false;
            let id = egui::ViewportId::from_hash_of(("tool", TOOLS[i].1));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        card
    }

    /// Каждая открытая функция живёт в своём отдельном окне.
    fn tool_windows(&mut self, ctx: &egui::Context) {
        for (i, &(tool, name, _)) in TOOLS.iter().enumerate() {
            if !self.tool_win[i] {
                continue;
            }
            let (w, h) = match tool {
                Tool::Sounds => (780.0, 740.0),
                Tool::Calc => (400.0, 700.0),
                Tool::Random => (480.0, 420.0),
                Tool::Wheel => (700.0, 820.0),
                Tool::History => (600.0, 680.0),
                Tool::Passwords => (560.0, 720.0),
                Tool::Settings => (640.0, 780.0),
                Tool::Downloads => (600.0, 560.0),
            };
            let id = egui::ViewportId::from_hash_of(("tool", name));
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("Petal — {name}"))
                .with_inner_size([w, h])
                .with_min_inner_size([320.0, 300.0]);
            let mut close = false;
            ctx.show_viewport_immediate(id, builder, |ctx, _class| {
                if tool == Tool::Sounds {
                    self.sound_keys(ctx);
                }
                egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(14))).show(ctx, |ui| {
                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match tool {
                        Tool::Sounds => self.sounds_ui(ui),
                        Tool::Calc => self.calc_ui(ui),
                        Tool::Random => self.random_ui(ui),
                        Tool::Wheel => self.wheel_window_ui(ui),
                        Tool::History => self.history_ui(ui),
                        Tool::Passwords => self.passwords_ui(ui),
                        Tool::Settings => self.settings_ui(ui),
                        Tool::Downloads => self.downloads_ui(ui),
                    });
                });
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            });
            if close {
                self.tool_win[i] = false;
                if tool == Tool::Passwords {
                    self.vault_lock();
                }
            }
        }
    }


    /// Меню с тремя точками: инкогнито, история, пароли, настройки.
    fn dots_menu(&mut self, ctx: &egui::Context) -> Option<egui::Rect> {
        let mut picked: Option<usize> = None;
        let mut incognito = false;
        let wins = self.tool_win;
        let card = popup_card(ctx, "dots", self.dots_open, self.menu_pos, self.bcfg.sidebar_w as f32 - 20.0, false, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if menu_item(ui, "Инкогнито", PURPLE) {
                incognito = true;
            }
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            for (i, (tool, name, color)) in TOOLS.iter().enumerate() {
                if in_dots(*tool) && menu_item(ui, name, if wins[i] { *color } else { TEXT }) {
                    picked = Some(i);
                }
            }
        });
        if incognito {
            self.open_private_tab();
            self.dots_open = false;
        }
        if let Some(i) = picked {
            self.tool_win[i] = true;
            self.dots_open = false;
            let id = egui::ViewportId::from_hash_of(("tool", TOOLS[i].1));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        card
    }

    /// Список загрузок: карточка вырастает вверх из кнопки «Загрузки».
    fn downloads_menu(&mut self, ctx: &egui::Context) -> Option<egui::Rect> {
        let now = store::now_secs();
        let time = ctx.input(|i| i.time) as f32;
        let mut clear = false;
        let downloads = &self.downloads;
        let card = popup_card(ctx, "downloads", self.dl_open, self.dl_pos, self.bcfg.sidebar_w as f32 - 20.0, true, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Загрузки").size(15.0).strong().color(TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !downloads.is_empty() && chip(ui, "Очистить").clicked() {
                        clear = true;
                    }
                });
            });
            if downloads.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("Пока ничего не скачано.").size(13.0).color(MUTED));
            }
            ScrollArea::vertical().max_height(330.0).auto_shrink([true, true]).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for d in downloads.iter().rev().take(40) {
                    let exists = Path::new(&d.path).exists();
                    Frame::new().fill(Color32::from_rgb(26, 26, 36)).corner_radius(CornerRadius::same(13)).inner_margin(Margin::same(9)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(RichText::new(d.name()).size(14.0).color(link())).truncate());
                        let status = match d.state {
                            store::DL_ACTIVE => "скачивается…".to_string(),
                            store::DL_FAILED => "не удалось скачать".to_string(),
                            _ if !exists => "файл удалён или перемещён".to_string(),
                            _ => store::ago(now, d.at),
                        };
                        ui.label(RichText::new(status).size(12.0).color(MUTED));
                        if d.state == store::DL_ACTIVE {
                            // Бегущая полоска: размер файла заранее неизвестен.
                            let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 5.0), Sense::hover());
                            ui.painter().rect_filled(bar, CornerRadius::same(3), Color32::from_rgb(44, 44, 60));
                            let w = bar.width() * 0.3;
                            let x = bar.left() + (bar.width() - w) * (0.5 + 0.5 * (time * 3.0).sin());
                            ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(x, bar.top()), Vec2::new(w, 5.0)), CornerRadius::same(3), GREEN);
                            ui.ctx().request_repaint();
                        }
                        if d.state == store::DL_DONE && exists {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 5.0;
                                if store::is_risky_file(&d.path) {
                                    ui.label(RichText::new("Запускайте сами").size(12.0).color(YELLOW));
                                } else if chip(ui, "Открыть").clicked() {
                                    open_path(&d.path);
                                }
                                if chip(ui, "В папке").clicked() {
                                    reveal_path(&d.path);
                                }
                            });
                        }
                    });
                }
            });
        });
        if clear {
            self.downloads.clear();
            store::save_downloads(&self.downloads);
        }
        card
    }

    /// Выпадающие меню; закрываются по Esc и по щелчку мимо них.
    fn menus(&mut self, ctx: &egui::Context) {
        let a = self.tools_menu(ctx);
        let b = self.dots_menu(ctx);
        let c = self.downloads_menu(ctx);
        let a = a.or(c);
        if self.tools_open || self.dots_open || self.dl_open {
            let (pressed, pos, esc) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos(), i.key_pressed(Key::Escape)));
            let inside = |p: egui::Pos2| [a, b].iter().flatten().any(|r| r.contains(p)) || self.menu_btns.iter().any(|r| r.contains(p)) || self.dl_btn.contains(p);
            if esc || (pressed && pos.is_some_and(|p| !inside(p))) {
                self.tools_open = false;
                self.dots_open = false;
                self.dl_open = false;
            }
        }
    }

    /// Страница ушла на весь экран (кнопка «развернуть» у видео): прячем панели и разворачиваем окно,
    /// чтобы по краям не оставалось полос. Выход из полноэкранного режима страницы возвращает всё назад.
    fn update_page_fullscreen(&mut self, ctx: &egui::Context) {
        let want = self.tabs[self.active].fs.load(Ordering::Relaxed);
        if want && !self.page_fs {
            self.page_fs = true;
            self.fs_url = self.tabs[self.active].current_url();
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        } else if self.page_fs {
            // Запасной выход: если ушли на другой сайт, а сообщения о выходе не пришло, не остаёмся без панелей.
            let moved = host_of(&self.tabs[self.active].current_url()) != host_of(&self.fs_url);
            if !want || moved {
                self.page_fs = false;
                self.tabs[self.active].fs.store(false, Ordering::Relaxed);
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
        }
    }

    /// Видео в маленьком окне: окно становится крошечным, всегда сверху, а страница показывает только видео.
    fn update_mini_player(&mut self, ctx: &egui::Context) {
        let want = self.tabs[self.active].pip.load(Ordering::Relaxed);
        if want && self.mini.is_none() {
            let size = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size())).unwrap_or(egui::vec2(1200.0, 800.0));
            self.mini = Some((host_of(&self.tabs[self.active].current_url()), size));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(480.0, 270.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        } else if let Some((host, size)) = self.mini.clone() {
            let moved = host_of(&self.tabs[self.active].current_url()) != host;
            if !want || moved {
                self.mini = None;
                self.tabs[self.active].pip.store(false, Ordering::Relaxed);
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            }
        }
    }

    fn open_private_tab(&mut self) {
        self.tabs.push(Pane::private(incognito_url()));
        self.select_tab(self.tabs.len() - 1);
    }

    // ── сессия и история ──

    /// Раз в секунду: пишет историю и список открытых вкладок, чтобы при закрытии ничего не потерялось.
    fn persist_tick(&mut self) {
        if self.persist_at.elapsed() < Duration::from_millis(1000) {
            return;
        }
        self.persist_at = Instant::now();
        self.history_tick();
        self.flush(false);
    }

    fn history_tick(&mut self) {
        if !self.bcfg.save_history {
            return;
        }
        for i in 0..self.tabs.len() {
            if self.tabs[i].incognito || self.tabs[i].view.is_none() {
                continue;
            }
            let url = self.tabs[i].url().unwrap_or_default();
            let title = self.tabs[i].title_text();
            let changed = {
                let t = &mut self.tabs[i];
                let changed = store::is_loggable(&url) && url != t.logged_url;
                if changed {
                    t.logged_url = url.clone();
                    t.url_changed_at = Instant::now();
                }
                changed
            };
            if changed {
                self.history.push(store::Visit { at: store::now_secs(), url, title: String::new() });
                self.tabs[i].hist_idx = Some(self.history.len() - 1);
                self.history_dirty = true;
            }
            // Название страницы приходит позже адреса, поэтому дописываем его с задержкой.
            let t = &mut self.tabs[i];
            if let Some(ix) = t.hist_idx {
                if t.url_changed_at.elapsed() > Duration::from_millis(1500) {
                    t.hist_idx = None;
                    if !title.is_empty() {
                        if let Some(v) = self.history.get_mut(ix) {
                            if v.url == t.logged_url && v.title.is_empty() {
                                v.title = title;
                                self.history_dirty = true;
                            }
                        }
                    }
                }
            }
        }
        if self.history.len() > store::MAX_HISTORY {
            store::trim_history(&mut self.history);
            for t in &mut self.tabs {
                t.hist_idx = None;
            }
        }
    }

    /// Папки в тех индексах, под которыми вкладки попадут в файл сессии (без инкогнито).
    fn group_specs(&self) -> Vec<store::GroupSpec> {
        let mut out: Vec<store::GroupSpec> = Vec::new();
        let mut cur: Option<u32> = None;
        for (n, t) in self.tabs.iter().filter(|t| !t.incognito).enumerate() {
            match (t.group, cur) {
                (Some(g), Some(c)) if g == c => {
                    if let Some(l) = out.last_mut() {
                        l.len += 1;
                    }
                }
                (Some(g), _) => {
                    if let Some(gr) = self.tab_groups.iter().find(|x| x.id == g) {
                        out.push(store::GroupSpec { name: gr.name.clone(), color: gr.color, collapsed: gr.collapsed, start: n, len: 1 });
                    }
                }
                _ => {}
            }
            cur = t.group;
        }
        out
    }

    fn session_text(&self) -> String {
        let mut urls = Vec::new();
        let mut active = 0;
        for (i, t) in self.tabs.iter().enumerate() {
            if t.incognito {
                continue;
            }
            if i == self.active {
                active = urls.len();
            }
            urls.push(t.current_url());
        }
        store::session_to_text(&urls, active)
    }

    /// Записывает вкладки и историю на диск, если они изменились (`force` пишет сразу, без паузы).
    fn flush(&mut self, force: bool) {
        if self.bcfg.restore {
            let text = self.session_text();
            if text != self.last_session {
                let _ = std::fs::write(store::SESSION_FILE, &text);
                self.last_session = text;
            }
        }
        if self.bcfg.restore {
            let g = self.group_specs();
            let text = store::groups_to_text(&g);
            if text != self.last_groups {
                if g.is_empty() {
                    let _ = std::fs::remove_file(store::GROUPS_FILE);
                } else {
                    let _ = std::fs::write(store::GROUPS_FILE, &text);
                }
                self.last_groups = text;
            }
        }
        if self.history_dirty && (force || self.history_saved_at.elapsed() > Duration::from_secs(3)) {
            store::save_history(&self.history);
            self.history_dirty = false;
            self.history_saved_at = Instant::now();
        }
        if self.bcfg != self.saved_bcfg {
            store::save_cfg(&self.bcfg);
            self.saved_bcfg = self.bcfg.clone();
        }
    }

    // ── загрузки ──

    fn drain_downloads(&mut self) {
        let events = match DL_EVENTS.lock() {
            Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
            _ => return,
        };
        for ev in events {
            match ev {
                DlEvent::Started { url, path } => {
                    self.downloads.push(store::Download {
                        at: store::now_secs(),
                        state: store::DL_ACTIVE,
                        path: path.to_string_lossy().to_string(),
                        url,
                    });
                }
                DlEvent::Done { url, path, ok } => {
                    if let Some(d) = self.downloads.iter_mut().rev().find(|d| d.url == url && d.state == store::DL_ACTIVE) {
                        d.state = if ok { store::DL_DONE } else { store::DL_FAILED };
                        if let Some(p) = path.filter(|p| !p.as_os_str().is_empty()) {
                            d.path = p.to_string_lossy().to_string();
                        }
                    }
                }
            }
        }
        store::save_downloads(&self.downloads);
    }

    fn downloads_ui(&mut self, ui: &mut egui::Ui) {
        header(ui, "Загрузки", "Файлы, скачанные в Petal. Папка загрузок обычная, «Загрузки» в Windows.");
        if !self.downloads.is_empty() && pill_button(ui, "Очистить список").clicked() {
            self.downloads.clear();
            store::save_downloads(&self.downloads);
        }
        ui.add_space(8.0);
        if self.downloads.is_empty() {
            ui.label(RichText::new("Пока ничего не скачано.").color(MUTED));
        }
        let now = store::now_secs();
        for d in self.downloads.iter().rev().take(100) {
            card(ui, |ui| {
                ui.label(RichText::new(d.name()).size(16.0).color(link()));
                let exists = Path::new(&d.path).exists();
                let status = match d.state {
                    store::DL_ACTIVE => "скачивается…".to_string(),
                    store::DL_FAILED => "не удалось скачать".to_string(),
                    _ if !exists => "файл удалён или перемещён".to_string(),
                    _ => store::ago(now, d.at),
                };
                ui.label(RichText::new(status).size(13.0).color(MUTED));
                if d.state == store::DL_DONE && exists {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if store::is_risky_file(&d.path) {
                            ui.label(RichText::new("Программы запускайте сами из папки.").size(13.0).color(YELLOW));
                        } else if pill_button(ui, "Открыть").clicked() {
                            open_path(&d.path);
                        }
                        if pill_button(ui, "Показать в папке").clicked() {
                            reveal_path(&d.path);
                        }
                    });
                }
            });
            ui.add_space(6.0);
        }
    }

    // ── окно «История» ──

    fn history_ui(&mut self, ui: &mut egui::Ui) {
        header(ui, "История", "Страницы, которые вы открывали. В режиме инкогнито история не пишется.");
        let mut open: Option<String> = None;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.hist_search)
                    .hint_text("Поиск по названию и адресу")
                    .desired_width(ui.available_width() - 190.0),
            );
            if self.hist_confirm_clear {
                if accent_button(ui, "Точно очистить?").clicked() {
                    self.history.clear();
                    for t in &mut self.tabs {
                        t.hist_idx = None;
                    }
                    self.history_dirty = true;
                    self.hist_confirm_clear = false;
                }
            } else if pill_button(ui, "Очистить историю").clicked() {
                self.hist_confirm_clear = true;
            }
        });
        ui.add_space(8.0);
        let q = self.hist_search.trim().to_lowercase();
        let now = store::now_secs();
        let mut shown = 0;
        for v in self.history.iter().rev() {
            if !q.is_empty() && !v.url.to_lowercase().contains(&q) && !v.title.to_lowercase().contains(&q) {
                continue;
            }
            if shown >= 300 {
                ui.label(RichText::new("Показаны последние 300 записей, уточните поиск.").color(MUTED));
                break;
            }
            shown += 1;
            let title = if v.title.is_empty() { host_of(&v.url).unwrap_or_else(|| v.url.clone()) } else { v.title.clone() };
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 50.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(10), CARD_BG);
            }
            let clip = ui.painter().with_clip_rect(rect.shrink2(Vec2::new(8.0, 0.0)));
            clip.text(rect.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, title, FontId::proportional(15.0), link());
            clip.text(
                rect.left_top() + Vec2::new(10.0, 28.0),
                Align2::LEFT_TOP,
                format!("{}  ·  {}", store::ago(now, v.at), v.url),
                FontId::proportional(12.5),
                MUTED,
            );
            if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                open = Some(v.url.clone());
            }
        }
        if shown == 0 {
            ui.label(RichText::new("Пока пусто.").color(MUTED));
        }
        if let Some(u) = open {
            self.open_tab(&u);
            ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
    }

    // ── окно «Настройки» ──

    /// Нужны ли изменения, которые вступают в силу только после перезапуска.
    fn restart_needed(&self) -> bool {
        let Some(f) = FROZEN.get() else { return false };
        let c = &self.bcfg;
        (c.doh.as_str(), c.doh_custom.as_str(), c.proxy.as_str(), c.ua.as_str(), c.lang.as_str())
            != (f.doh.as_str(), f.doh_custom.as_str(), f.proxy.as_str(), f.ua.as_str(), f.lang.as_str())
            || c.no_gpu != f.no_gpu
            || c.webrtc_safe != f.webrtc_safe
            || c.dark_sites != f.dark_sites
            || c.privacy_signals != f.privacy_signals
            || c.extensions != f.extensions
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        header(ui, "Настройки", "Браузер Petal");
        if self.restart_needed() {
            card(ui, |ui| {
                ui.label(RichText::new("Часть изменений вступит в силу после перезапуска Petal: закройте и откройте программу.").color(YELLOW));
            });
        }

        card(ui, |ui| {
            section(ui, "Оформление");
            ui.label(RichText::new("Цветовая тема").color(MUTED));
            ui.add_space(6.0);
            let mut picked: Option<String> = None;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(10.0);
                let mut swatch = |ui: &mut egui::Ui, id: &str, a: u32, b: u32, mark: &str| {
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(46.0), Sense::click());
                    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                    let t = hover_t(ui, &resp);
                    let sel = ui.ctx().abt(resp.id.with("sel"), self.bcfg.theme == id, 0.22);
                    let p = ui.painter();
                    p.circle_filled(r.center() + Vec2::new(0.0, 3.0), 23.0 + 2.0 * t, alpha(rgb(a), (30.0 + 50.0 * t) as u8));
                    grad_rrect(p, r.expand(1.5 * t), 23.0, rgb(a), rgb(b), false);
                    if !mark.is_empty() {
                        p.text(r.center(), Align2::CENTER_CENTER, mark, FontId::proportional(20.0), Color32::from_rgba_unmultiplied(255, 255, 255, 220));
                    }
                    if sel > 0.01 {
                        p.circle_stroke(r.center(), 27.0, Stroke::new(2.0_f32, alpha(Color32::WHITE, (230.0 * sel) as u8)));
                        p.circle_filled(r.center(), 5.0 * sel, Color32::WHITE);
                    }
                    if resp.clicked() {
                        picked = Some(id.to_string());
                    }
                };
                for (id, _, a, b) in store::THEMES.iter() {
                    swatch(ui, id, *a, *b, "");
                }
                let (ca, cb) = if self.bcfg.theme == "custom" { (self.bcfg.accent_a, self.bcfg.accent_b) } else { (0x3A3A46, 0x56566A) };
                swatch(ui, "custom", ca, cb, if self.bcfg.theme == "custom" { "" } else { "+" });
            });
            if let Some(id) = picked {
                if id == "custom" && self.bcfg.theme != "custom" {
                    let (a, b) = store::theme_colors(&self.bcfg);
                    self.bcfg.accent_a = a;
                    self.bcfg.accent_b = b;
                }
                self.bcfg.theme = id;
            }
            let name = if self.bcfg.theme == "custom" { "Своя тема" } else { store::THEMES.iter().find(|t| t.0 == self.bcfg.theme).map(|t| t.1).unwrap_or("") };
            ui.add_space(4.0);
            ui.label(RichText::new(name).size(14.0).color(TEXT));
            if self.bcfg.theme == "custom" {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for (label, v) in [("Цвет 1", &mut self.bcfg.accent_a), ("Цвет 2", &mut self.bcfg.accent_b)] {
                        ui.label(RichText::new(label).color(MUTED));
                        let mut c = [(*v >> 16) as u8, (*v >> 8) as u8, *v as u8];
                        if egui::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
                            *v = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
                        }
                        ui.add_space(10.0);
                    }
                });
            }
            // Полоска-пример: так выглядят акценты (кнопки, подсветка, вкладки).
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), Sense::hover());
            grad_rrect(ui.painter(), r, 5.0, accent_a(), accent_b(), false);
            ui.add_space(12.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new("Размер интерфейса").color(MUTED));
                ui.add(egui::Slider::new(&mut self.bcfg.ui_scale, 80..=140).suffix("%").step_by(5.0));
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Ширина левой панели").color(MUTED));
                ui.add(egui::Slider::new(&mut self.bcfg.sidebar_w, 240..=340).suffix(" px").step_by(10.0));
            });
            ui.add_space(4.0);
            ui.checkbox(&mut self.bcfg.anim, "Анимации (интерфейс и новая вкладка)");
            ui.checkbox(&mut self.bcfg.glow, "Светящиеся пятна за левой панелью");
            ui.checkbox(&mut self.bcfg.compact_tabs, "Компактные вкладки");
            ui.add_space(8.0);
            let walls: Vec<(&str, &str)> = store::WALLPAPERS.to_vec();
            combo(ui, "wall", "Фон новой вкладки", &walls, &mut self.bcfg.wallpaper);
            if self.bcfg.wallpaper == "image" {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if pill_button(ui, "Выбрать картинку…").clicked() {
                        if let Some(f) = rfd::FileDialog::new().add_filter("Картинки", &["png", "jpg", "jpeg"]).pick_file() {
                            self.bcfg.wallpaper_img = f.display().to_string();
                        }
                    }
                    let shown = Path::new(&self.bcfg.wallpaper_img).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "не выбрана".to_string());
                    ui.label(RichText::new(shown).size(13.0).color(MUTED));
                });
                note(ui, "PNG или JPG до 6 МБ. Картинка копируется в страницу, исходный файл можно потом переместить.");
            }
            ui.add_space(8.0);
            ui.checkbox(&mut self.bcfg.clock, "Часы и приветствие на новой вкладке");
            ui.add_space(4.0);
            ui.label(RichText::new("Ваше имя для приветствия").color(MUTED));
            ui.add(egui::TextEdit::singleline(&mut self.bcfg.user_name).hint_text("Необязательно").desired_width(260.0));
        });

        card(ui, |ui| {
            section(ui, "Основные");
            ui.checkbox(&mut self.bcfg.restore, "Открывать при запуске вкладки, которые были при закрытии");
            ui.checkbox(&mut self.bcfg.save_history, "Записывать историю посещений");
            ui.checkbox(&mut self.bcfg.clear_on_exit, "Стирать cookie и кэш сайтов при каждом запуске");
            note(ui, "С этим пунктом из аккаунтов на сайтах придётся входить заново.");
            ui.checkbox(&mut self.bcfg.wipe_incognito, "Стирать данные инкогнито, когда закрыта последняя такая вкладка");
            note(ui, "Cookie и кэш приватных вкладок удаляются сразу, загрузки из них не попадают в список.");
        });

        card(ui, |ui| {
            section(ui, "Обновления");
            ui.checkbox(&mut self.bcfg.auto_update, "Проверять обновления при запуске");
            ui.horizontal(|ui| {
                if pill_button(ui, "Проверить сейчас").clicked() {
                    self.bcfg.skip_version.clear();
                    self.start_update_check(true);
                }
                if self.upd.is_some() && pill_button(ui, "Обновить").clicked() {
                    self.upd_hidden = false;
                    self.start_update();
                }
            });
            if !self.upd_msg.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(&self.upd_msg).size(13.0).color(GREEN));
            }
            note(ui, &format!("Сейчас установлена версия {APP_VERSION}. Установленная программа обновляется сама: скачивается новый установщик, Petal закрывается и запускается снова. Переносную версию нужно скачать со страницы релиза."));
        });

        card(ui, |ui| {
            section(ui, "Поиск и новая вкладка");
            let opts: Vec<(&str, &str)> = store::SEARCH_ENGINES.iter().map(|e| (e.0, e.1)).collect();
            combo(ui, "search", "Поисковая система", &opts, &mut self.bcfg.search);
            ui.add_space(6.0);
            ui.label(RichText::new("Страница новой вкладки").color(MUTED));
            ui.add(egui::TextEdit::singleline(&mut self.bcfg.home).desired_width(f32::INFINITY));
            note(ui, "Адрес google.com или пустое поле открывают стартовую страницу Petal с закладками.");
        });

        card(ui, |ui| {
            section(ui, "Сеть и DNS");
            let mut opts: Vec<(&str, &str)> = store::DOH_SERVERS.iter().map(|e| (e.0, e.1)).collect();
            opts.push(("custom", "Свой сервер…"));
            combo(ui, "doh", "DNS-сервер (защищённый, DoH)", &opts, &mut self.bcfg.doh);
            if self.bcfg.doh == "custom" {
                ui.add_space(4.0);
                ui.add(egui::TextEdit::singleline(&mut self.bcfg.doh_custom).hint_text("https://dns.example/dns-query").desired_width(f32::INFINITY));
                if store::doh_template(&self.bcfg).is_none() {
                    ui.label(RichText::new("Адрес должен начинаться с https:// и не содержать пробелов.").size(13.0).color(RED));
                }
            }
            note(ui, "Экспериментально: если ваша версия движка не поддерживает такую настройку, сайты откроются через обычный DNS Windows. Проверить, включился ли DoH, можно кнопкой ниже (страница проверки Cloudflare).");
            if pill_button(ui, "Проверить DNS").clicked() {
                self.open_tab("https://1.1.1.1/help");
            }
            ui.add_space(10.0);
            ui.label(RichText::new("Прокси-сервер").color(MUTED));
            ui.add(egui::TextEdit::singleline(&mut self.bcfg.proxy).hint_text("http://127.0.0.1:8080 или socks5://127.0.0.1:1080").desired_width(f32::INFINITY));
            if !self.bcfg.proxy.trim().is_empty() && store::proxy_arg(&self.bcfg).is_empty() {
                ui.label(RichText::new("Формат: http://адрес:порт или socks5://адрес:порт.").size(13.0).color(RED));
            }
            ui.add_space(6.0);
            combo(ui, "lang", "Язык сайтов", &[("", "Как в Windows"), ("ru", "Русский"), ("en", "English")], &mut self.bcfg.lang);
        });

        card(ui, |ui| {
            section(ui, "Расширения Chrome");
            ui.checkbox(&mut self.bcfg.extensions, "Включить расширения");
            note(ui, "Работают расширения, которые не требуют входа в аккаунт Google. Кнопок расширений на панели нет: они действуют прямо на страницах (блокировщики рекламы, тёмные темы, переводчики). Часть расширений может работать не полностью.");
            ui.add_space(8.0);
            ui.label(RichText::new("Ссылка на расширение в Chrome Web Store или его идентификатор").color(MUTED));
            ui.add(egui::TextEdit::singleline(&mut self.ext_input).hint_text("https://chromewebstore.google.com/detail/…").desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                let busy = self.ext_rx.is_some();
                if pill_button(ui, if busy { "Скачиваю…" } else { "Установить" }).clicked() && !busy {
                    match store::ext_id_from(&self.ext_input) {
                        Some(id) => {
                            let (tx, rx) = mpsc::channel();
                            self.ext_rx = Some(rx);
                            self.ext_status = "Скачиваю расширение…".to_string();
                            std::thread::spawn(move || {
                                let _ = tx.send(install_extension(&id));
                            });
                            self.ext_input.clear();
                            self.bcfg.extensions = true;
                        }
                        None => self.ext_status = "В ссылке не нашёлся идентификатор расширения (32 буквы). Скопируйте адрес страницы расширения целиком.".to_string(),
                    }
                }
                if pill_button(ui, "Открыть Chrome Web Store").clicked() {
                    self.open_tab("https://chromewebstore.google.com/");
                }
            });
            if !self.ext_status.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(&self.ext_status).size(13.0).color(GREEN));
            }
            if !self.ext_list.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Установлено").color(MUTED));
                let mut remove: Option<String> = None;
                for (dir, name) in &self.ext_list {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).color(TEXT));
                        if chip(ui, "Удалить").clicked() {
                            remove = Some(dir.clone());
                        }
                    });
                }
                if let Some(d) = remove {
                    let _ = std::fs::remove_dir_all(extensions_dir().join(&d));
                    self.ext_list = list_extensions();
                    self.ext_status = "Расширение удалено. Изменение вступит в силу после перезапуска.".to_string();
                }
            }
        });

        card(ui, |ui| {
            section(ui, "Приватность");
            ui.checkbox(&mut self.bcfg.privacy_signals, "Просить сайты не отслеживать меня (DNT и GPC)");
            ui.checkbox(&mut self.bcfg.block_popups, "Блокировать всплывающие окна сайтов");
            note(ui, "Из-за этого может не открыться вход через Google или VK: они работают во всплывающем окне.");
            ui.checkbox(&mut self.bcfg.webrtc_safe, "Не раскрывать настоящий IP-адрес через WebRTC");
            ui.add_space(6.0);
            combo(
                ui,
                "ua",
                "Как представляться сайтам",
                &[("default", "Как есть (Edge)"), ("chrome", "Google Chrome"), ("firefox", "Firefox"), ("mobile", "Телефон Android")],
                &mut self.bcfg.ua,
            );
        });

        card(ui, |ui| {
            section(ui, "Вид и скорость");
            ui.horizontal(|ui| {
                ui.label(RichText::new("Масштаб страниц").color(MUTED));
                ui.add(egui::Slider::new(&mut self.bcfg.zoom, 50..=200).suffix("%").step_by(5.0));
                if chip(ui, "100%").clicked() {
                    self.bcfg.zoom = 100;
                }
            });
            ui.checkbox(&mut self.bcfg.dark_sites, "Тёмная тема для сайтов, которые её поддерживают");
            ui.checkbox(&mut self.bcfg.no_gpu, "Отключить аппаратное ускорение (если страницы мерцают или зависают)");
        });

        card(ui, |ui| {
            section(ui, "Загрузки");
            ui.label(RichText::new("Папка для скачанных файлов").color(MUTED));
            ui.add(egui::TextEdit::singleline(&mut self.bcfg.dl_dir).hint_text("Пусто: обычная папка «Загрузки»").desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                if pill_button(ui, "Выбрать папку…").clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        self.bcfg.dl_dir = d.display().to_string();
                    }
                }
                if !self.bcfg.dl_dir.is_empty() && pill_button(ui, "Сбросить").clicked() {
                    self.bcfg.dl_dir.clear();
                }
            });
        });

        card(ui, |ui| {
            section(ui, "Данные");
            ui.horizontal_wrapped(|ui| {
                if pill_button(ui, "Очистить историю").clicked() {
                    self.history.clear();
                    for t in &mut self.tabs {
                        t.hist_idx = None;
                    }
                    self.history_dirty = true;
                    self.settings_status = "История очищена.".to_string();
                }
                if pill_button(ui, "Очистить cookie и кэш сайтов").clicked() {
                    for t in &self.tabs {
                        if let Some(b) = &t.view {
                            b.clear_data();
                        }
                    }
                    self.settings_status = "Данные сайтов очищены: из аккаунтов на сайтах придётся войти заново.".to_string();
                }
                if pill_button(ui, "Сбросить настройки").clicked() {
                    self.bcfg = store::BrowserCfg::default();
                    self.settings_status = "Настройки сброшены к исходным.".to_string();
                }
            });
            if !self.settings_status.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&self.settings_status).color(GREEN));
            }
            ui.add_space(6.0);
            note(
                ui,
                &format!(
                    "Petal {}. Данные лежат в {}. Вкладки инкогнито ни в историю, ни в сессию не попадают.",
                    APP_VERSION,
                    std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default()
                ),
            );
        });
    }

    // ── окно «Пароли» ──

    fn vault_lock(&mut self) {
        if let Some(k) = self.vault.key.as_mut() {
            k.fill(0);
        }
        self.vault.key = None;
        self.vault.entries.clear();
        self.vault.pw.clear();
        self.vault.pw2.clear();
        self.vault.pass.clear();
        self.vault.shown.clear();
        self.vault.confirm_delete = None;
    }

    /// Шифрует и записывает пароли; сначала во временный файл, чтобы сбой не испортил старый.
    fn vault_save(&mut self) {
        let Some(key) = self.vault.key else { return };
        let bytes = store::vault_encrypt(&self.vault.entries, &key, &self.vault.salt);
        let tmp = format!("{}.tmp", store::VAULT_FILE);
        let res = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, store::VAULT_FILE));
        self.vault.status = match res {
            Ok(()) => String::new(),
            Err(e) => format!("Не удалось сохранить: {e}"),
        };
        self.vault.exists = true;
    }

    fn vault_unlock(&mut self) {
        let bytes = match std::fs::read(store::VAULT_FILE) {
            Ok(b) => b,
            Err(e) => {
                self.vault.status = format!("Не удалось прочитать файл: {e}");
                return;
            }
        };
        let salt = match store::vault_salt(&bytes) {
            Ok(s) => s,
            Err(_) => {
                self.vault.status = "Файл с паролями повреждён.".to_string();
                return;
            }
        };
        let key = store::derive_key(&self.vault.pw, &salt);
        match store::vault_decrypt(&bytes, &key) {
            Ok(entries) => {
                self.vault.salt = salt;
                self.vault.key = Some(key);
                self.vault.entries = entries;
                self.vault.pw.clear();
                self.vault.status.clear();
            }
            Err(store::VaultError::WrongPassword) => self.vault.status = "Неверный мастер-пароль.".to_string(),
            Err(store::VaultError::Corrupt) => self.vault.status = "Файл с паролями повреждён.".to_string(),
        }
    }

    fn vault_create(&mut self) {
        if self.vault.pw.chars().count() < 6 {
            self.vault.status = "Мастер-пароль должен быть не короче 6 символов.".to_string();
        } else if self.vault.pw != self.vault.pw2 {
            self.vault.status = "Пароли не совпадают.".to_string();
        } else {
            self.vault.salt = store::new_salt();
            self.vault.key = Some(store::derive_key(&self.vault.pw, &self.vault.salt));
            self.vault.entries.clear();
            self.vault.pw.clear();
            self.vault.pw2.clear();
            self.vault_save();
        }
    }

    fn passwords_ui(&mut self, ui: &mut egui::Ui) {
        header(ui, "Пароли", "Лежат только на этом компьютере и зашифрованы мастер-паролем.");
        if self.vault.key.is_none() {
            self.vault_locked_ui(ui);
        } else {
            self.vault_open_ui(ui);
        }
    }

    fn vault_locked_ui(&mut self, ui: &mut egui::Ui) {
        let creating = !self.vault.exists;
        card(ui, |ui| {
            if creating {
                ui.label(RichText::new("Придумайте мастер-пароль").size(17.0).color(TEXT));
                ui.label(RichText::new("Забыв его, пароли восстановить нельзя: ни я, ни программа этого не умеют.").size(13.0).color(YELLOW));
            } else {
                ui.label(RichText::new("Введите мастер-пароль").size(17.0).color(TEXT));
            }
            ui.add_space(8.0);
            let first = ui.add(egui::TextEdit::singleline(&mut self.vault.pw).password(true).hint_text("Мастер-пароль").desired_width(f32::INFINITY));
            let mut enter = first.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if creating {
                ui.add_space(6.0);
                let second = ui.add(egui::TextEdit::singleline(&mut self.vault.pw2).password(true).hint_text("Повторите пароль").desired_width(f32::INFINITY));
                enter = enter || (second.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
            }
            ui.add_space(8.0);
            if accent_button(ui, if creating { "Создать хранилище" } else { "Открыть" }).clicked() || enter {
                if creating {
                    self.vault_create();
                } else {
                    self.vault_unlock();
                }
            }
            if !self.vault.status.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&self.vault.status).color(RED));
            }
        });
    }

    fn vault_open_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut lock = false;
        let mut copy: Option<String> = None;
        card(ui, |ui| {
            ui.label(RichText::new("Новая запись").size(17.0).color(TEXT));
            ui.add_space(6.0);
            ui.add(egui::TextEdit::singleline(&mut self.vault.site).hint_text("Сайт").desired_width(f32::INFINITY));
            ui.add_space(4.0);
            ui.add(egui::TextEdit::singleline(&mut self.vault.login).hint_text("Логин или почта").desired_width(f32::INFINITY));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.vault.pass).hint_text("Пароль").desired_width(ui.available_width() - 150.0));
                if pill_button(ui, "Сгенерировать").clicked() {
                    self.vault.pass = store::generate_password(16);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if accent_button(ui, "Добавить").clicked() {
                    if self.vault.site.trim().is_empty() || self.vault.pass.is_empty() {
                        self.vault.status = "Нужны хотя бы сайт и пароль.".to_string();
                    } else {
                        self.vault.entries.push(store::Entry {
                            site: self.vault.site.trim().to_string(),
                            login: self.vault.login.trim().to_string(),
                            password: std::mem::take(&mut self.vault.pass),
                        });
                        self.vault.site.clear();
                        self.vault.login.clear();
                        self.vault.status.clear();
                        changed = true;
                    }
                }
                if pill_button(ui, "Заблокировать").clicked() {
                    lock = true;
                }
            });
            if !self.vault.status.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&self.vault.status).color(RED));
            }
        });
        ui.add_space(10.0);
        if self.vault.entries.is_empty() {
            ui.label(RichText::new("Записей пока нет.").color(MUTED));
        }
        let mut delete: Option<usize> = None;
        for (i, e) in self.vault.entries.iter().enumerate() {
            card(ui, |ui| {
                ui.label(RichText::new(&e.site).size(16.0).color(link()));
                ui.label(RichText::new(if e.login.is_empty() { "(без логина)" } else { &e.login }).color(MUTED));
                let shown = self.vault.shown.contains(&i);
                ui.label(RichText::new(if shown { e.password.clone() } else { "••••••••••".to_string() }).monospace().color(TEXT));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if pill_button(ui, "Копировать логин").clicked() {
                        copy = Some(e.login.clone());
                    }
                    if pill_button(ui, "Копировать пароль").clicked() {
                        copy = Some(e.password.clone());
                    }
                    if pill_button(ui, if shown { "Скрыть" } else { "Показать" }).clicked() {
                        if shown {
                            self.vault.shown.remove(&i);
                        } else {
                            self.vault.shown.insert(i);
                        }
                    }
                    let sure = self.vault.confirm_delete == Some(i);
                    if pill_button(ui, if sure { "Точно удалить?" } else { "Удалить" }).clicked() {
                        if sure {
                            delete = Some(i);
                        } else {
                            self.vault.confirm_delete = Some(i);
                        }
                    }
                });
            });
            ui.add_space(6.0);
        }
        if let Some(i) = delete {
            self.vault.entries.remove(i);
            self.vault.shown.clear();
            self.vault.confirm_delete = None;
            changed = true;
        }
        if let Some(t) = copy {
            ui.ctx().copy_text(t);
        }
        if changed {
            self.vault_save();
        }
        if lock {
            self.vault_lock();
        }
    }

    // ── звук: настройка микрофона ──

    fn devices_refresh(&mut self) {
        self.outputs = list_output_names();
        self.inputs = list_input_names();
        self.devices_loaded = true;
        if self.cfg.cable.is_empty() {
            if let Some(c) = pick_cable(&self.outputs) {
                self.cfg.cable = c;
            }
        }
        if self.cfg.input.is_empty() {
            if let Some(i) = pick_input(&self.inputs) {
                self.cfg.input = i;
            }
        }
    }

    fn audio_key(&self) -> AudioKey {
        (self.cfg.mic_on, self.cfg.cable.clone(), self.cfg.pass_on, self.cfg.input.clone())
    }

    /// Приводит кабель и подмешивание голоса в соответствие с настройками (только при их смене).
    fn audio_sync(&mut self) {
        let key = self.audio_key();
        if self.applied.as_ref() == Some(&key) {
            return;
        }
        self.applied = Some(key);
        self.pass = None;
        self.cable = None;
        self.mic_status.clear();
        self.sound_stop_all();
        if !self.cfg.mic_on {
            return;
        }
        if self.cfg.cable.is_empty() {
            self.mic_status = "Выберите виртуальный кабель".to_string();
            return;
        }
        match open_output_by_name(&self.cfg.cable) {
            Ok(sink) => {
                if self.cfg.pass_on {
                    if self.cfg.input.is_empty() {
                        self.mic_status = "Выберите свой микрофон или отключите подмешивание голоса".to_string();
                    } else {
                        match start_passthrough(&self.cfg.input, &sink) {
                            Ok(p) => self.pass = Some(p),
                            Err(e) => self.mic_status = format!("Голос не подмешивается: {e}"),
                        }
                    }
                }
                self.cable = Some(sink);
            }
            Err(e) => self.mic_status = format!("Кабель не открылся: {e}"),
        }
    }

    fn audio_ui(&mut self, ui: &mut egui::Ui) {
        if !self.devices_loaded {
            self.devices_refresh();
        }
        let outputs = self.outputs.clone();
        let inputs = self.inputs.clone();
        let have_cable = outputs.iter().any(|n| is_cable_name(n));
        let mut open_site = false;
        let mut refresh = false;

        card(ui, |ui| {
            ui.label(RichText::new("Звук в микрофон").size(13.0).color(link()));
            ui.checkbox(&mut self.cfg.mic_on, "Играть звуки в микрофон (Discord, игры, звонки)");
            if !have_cable {
                ui.label(
                    RichText::new(
                        "Нужен виртуальный кабель: Windows не даёт программе самой стать микрофоном. Установите бесплатный VB-CABLE, перезагрузите компьютер и нажмите «Обновить устройства».",
                    )
                    .size(13.0)
                    .color(YELLOW),
                );
                if accent_button(ui, "Открыть сайт VB-CABLE").clicked() {
                    open_site = true;
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Микрофон, в который идут звуки:");
            });
            egui::ComboBox::from_id_salt("cable_out")
                .width(300.0)
                .selected_text(if self.cfg.cable.is_empty() { "не выбран" } else { &self.cfg.cable })
                .show_ui(ui, |ui| {
                    for n in &outputs {
                        let label = if is_cable_name(n) { format!("{n}  (виртуальный микрофон)") } else { n.clone() };
                        ui.selectable_value(&mut self.cfg.cable, n.clone(), label);
                    }
                });
            if !self.cfg.cable.is_empty() && !is_cable_name(&self.cfg.cable) {
                ui.label(
                    RichText::new("Это не виртуальный кабель: звуки пойдут просто на это устройство, а в Discord попадут, только если он слушает именно его.")
                        .size(13.0)
                        .color(YELLOW),
                );
            }
            if self.cfg.mic_on {
                ui.checkbox(&mut self.cfg.hear_self, "Слышать звуки и в своих наушниках");
                ui.horizontal(|ui| {
                    ui.label("Громкость в микрофоне");
                    let changed = ui.add(egui::Slider::new(&mut self.cfg.mic_volume, 0.0..=1.5).show_value(false)).changed();
                    if changed {
                        for p in &self.players {
                            if let Some(m) = &p.mic {
                                m.set_volume(self.cfg.mic_volume);
                            }
                        }
                    }
                    ui.label(RichText::new(format!("{:.0}%", self.cfg.mic_volume * 100.0)).color(MUTED));
                });
                ui.checkbox(&mut self.cfg.pass_on, "Подмешивать мой голос (иначе собеседники слышат только звуки)");
                if self.cfg.pass_on {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Мой микрофон");
                        egui::ComboBox::from_id_salt("mic_in")
                            .width(230.0)
                            .selected_text(if self.cfg.input.is_empty() { "не выбран" } else { &self.cfg.input })
                            .show_ui(ui, |ui| {
                                for n in &inputs {
                                    ui.selectable_value(&mut self.cfg.input, n.clone(), n);
                                }
                            });
                    });
                    if is_cable_name(&self.cfg.input) {
                        ui.label(RichText::new("Это похоже на сам кабель, голос пойдёт по кругу. Выберите свой настоящий микрофон.").size(13.0).color(RED));
                    }
                }
                ui.label(
                    RichText::new("В Discord или игре выберите микрофоном «CABLE Output» (выход кабеля).")
                        .size(13.0)
                        .color(MUTED),
                );
                if !self.mic_status.is_empty() {
                    ui.label(RichText::new(&self.mic_status).size(13.0).color(RED));
                } else if self.cable.is_some() {
                    ui.label(RichText::new("Кабель подключён").size(13.0).color(GREEN));
                }
            }
            if pill_button(ui, "Обновить устройства").clicked() {
                refresh = true;
            }
        });
        if refresh {
            self.devices_refresh();
        }
        if open_site {
            self.open_tab(VB_CABLE_URL);
        }
    }

    fn sound_set_volumes(&self) {
        for p in &self.players {
            if let Some(l) = &p.local {
                l.set_volume(self.cfg.volume);
            }
            if let Some(m) = &p.mic {
                m.set_volume(self.cfg.mic_volume);
            }
        }
    }

    fn sounds_reload(&mut self) {
        let dir = sounds_dir();
        if !self.sounds_loaded {
            self.hotkeys = load_hotkeys(&dir);
        }
        self.sounds = list_sounds(&dir);
        self.groups = list_groups(&dir);
        self.sounds_loaded = true;

        let before = self.hotkeys.len();
        let sounds = &self.sounds;
        self.hotkeys.retain(|rel, _| sounds.iter().any(|s| s.rel == *rel));
        if self.hotkeys.len() != before {
            save_hotkeys(&dir, &self.hotkeys);
        }
        if let Some(g) = &self.group_sel {
            if !g.is_empty() && !self.groups.contains(g) {
                self.group_sel = None;
            }
        }
        self.confirm_delete = None;
        self.rename = None;
    }

    fn sounds_add(&mut self, files: Vec<PathBuf>) {
        let dir = sounds_dir();
        let group = self.group_sel.clone().unwrap_or_default();
        let (mut ok, mut errors) = (0usize, Vec::new());
        for f in files {
            if f.is_dir() {
                let (n, e) = import_folder(&f, &dir);
                ok += n;
                errors.extend(e);
            } else if f.is_file() {
                match import_sound(&f, &dir, &group) {
                    Ok(_) => ok += 1,
                    Err(e) => errors.push(e),
                }
            }
        }
        self.sounds_reload();
        self.sound_status = match (ok, errors.is_empty()) {
            (0, true) => String::new(),
            (n, true) => format!("Добавлено звуков: {n}"),
            (0, false) => errors.join("\n"),
            (n, false) => format!("Добавлено звуков: {n}\n{}", errors.join("\n")),
        };
    }

    /// Переносит назначенную клавишу, когда звук переименовали или переместили.
    fn sound_rehome(&mut self, old_rel: &str, new_rel: &str) {
        if old_rel != new_rel {
            if let Some(k) = self.hotkeys.remove(old_rel) {
                self.hotkeys.insert(new_rel.to_string(), k);
                save_hotkeys(&sounds_dir(), &self.hotkeys);
            }
        }
    }

    fn sound_stop_all(&mut self) {
        for p in &self.players {
            p.stop();
        }
        self.players.clear();
    }

    /// Играющий файл удерживается открытым, а Windows не даёт менять открытый файл.
    fn sound_release(&mut self, rel: &str) {
        for p in self.players.iter().filter(|p| p.rel == rel) {
            p.stop();
        }
        self.players.retain(|p| p.rel != rel);
    }

    fn sound_play(&mut self, rel: &str) {
        let Some(sound) = self.sounds.iter().find(|s| s.rel == rel).cloned() else { return };
        let to_mic = self.cfg.mic_on && self.cable.is_some();
        // Если микрофон не готов, звук всё равно слышен локально, а причина видна в статусе.
        let to_local = self.cfg.hear_self || !to_mic;
        if self.cfg.mic_on && !to_mic {
            self.sound_status = if self.mic_status.is_empty() {
                "Микрофон не готов: настройте кабель ниже".to_string()
            } else {
                format!("Микрофон не готов: {}", self.mic_status)
            };
        } else {
            self.sound_status.clear();
        }
        if to_local && self.audio.is_none() {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(h) => self.audio = Some(h),
                Err(e) => {
                    self.sound_status = format!("Не удалось открыть звуковое устройство: {e}");
                    return;
                }
            }
        }
        if !self.cfg.overlap {
            self.sound_stop_all();
        }
        let mut local = None;
        let mut mic = None;
        let mut total = None;
        let mut error = None;
        if to_local {
            if let Some(h) = &self.audio {
                match start_player(h.mixer(), &sound.path, self.cfg.volume) {
                    Ok((p, t)) => {
                        local = Some(p);
                        total = t;
                    }
                    Err(e) => error = Some(e),
                }
            }
        }
        if to_mic {
            if let Some(h) = &self.cable {
                match start_player(h.mixer(), &sound.path, self.cfg.mic_volume) {
                    Ok((p, t)) => {
                        mic = Some(p);
                        total = total.or(t);
                    }
                    Err(e) => error = Some(e),
                }
            }
        }
        if let Some(e) = error {
            self.sound_status = format!("Не удалось проиграть «{}»: {e}", sound.name);
        }
        if local.is_some() || mic.is_some() {
            self.players.push(Playing { rel: sound.rel, local, mic, total });
        }
    }

    /// Нажатие на играющий звук останавливает его, на остальные запускает.
    fn sound_toggle(&mut self, rel: &str) {
        if self.players.iter().any(|p| p.rel == rel) {
            self.sound_release(rel);
        } else {
            self.sound_play(rel);
        }
    }

    fn sound_rename(&mut self, rel: &str, new_name: &str) {
        let Some(s) = self.sounds.iter().find(|s| s.rel == rel).cloned() else { return };
        self.sound_release(rel);
        match rename_sound(&s.path, new_name) {
            Ok(p) => {
                let new_rel = rel_path(&sounds_dir(), &p);
                self.sound_rehome(rel, &new_rel);
                self.sound_status = format!("Переименовано: {}", sound_name(&p));
            }
            Err(e) => self.sound_status = e,
        }
        self.sounds_reload();
    }

    fn sound_move(&mut self, rel: &str, group: &str) {
        let Some(s) = self.sounds.iter().find(|s| s.rel == rel).cloned() else { return };
        self.sound_release(rel);
        let dir = sounds_dir();
        match move_sound(&s.path, &dir, group) {
            Ok(p) => {
                let new_rel = rel_path(&dir, &p);
                self.sound_rehome(rel, &new_rel);
                self.sound_status = if group.is_empty() {
                    format!("«{}» перенесён из группы", s.name)
                } else {
                    format!("«{}» перенесён в группу «{group}»", s.name)
                };
            }
            Err(e) => self.sound_status = e,
        }
        self.sounds_reload();
    }

    fn sound_delete(&mut self, rel: &str) {
        let Some(s) = self.sounds.iter().find(|s| s.rel == rel).cloned() else { return };
        self.sound_release(rel);
        self.sound_status = match std::fs::remove_file(&s.path) {
            Ok(()) => {
                if self.hotkeys.remove(rel).is_some() {
                    save_hotkeys(&sounds_dir(), &self.hotkeys);
                }
                format!("Удалено: {}", s.name)
            }
            Err(e) => format!("Не удалось удалить «{}»: {e}", s.name),
        };
        self.sounds_reload();
    }

    fn group_delete(&mut self, name: &str) {
        self.sound_status = match std::fs::remove_dir(sounds_dir().join(name)) {
            Ok(()) => format!("Группа «{name}» удалена"),
            Err(_) => format!("В группе «{name}» ещё есть звуки: сначала перенесите или удалите их"),
        };
        self.sounds_reload();
    }

    fn sound_action(&mut self, rel: String, action: TileAction) {
        match action {
            TileAction::Toggle => self.sound_toggle(&rel),
            TileAction::Rename => {
                let name = self.sounds.iter().find(|s| s.rel == rel).map(|s| s.name.clone()).unwrap_or_default();
                self.rename = Some((rel, name, true));
            }
            TileAction::Hotkey => self.capture = Some(rel),
            TileAction::Move(group) => self.sound_move(&rel, &group),
            TileAction::Delete => self.confirm_delete = Some(rel),
        }
    }

    /// Клавиши саундпада: назначение новой клавиши или запуск звука по нажатию.
    fn sound_keys(&mut self, ctx: &egui::Context) {
        let presses: Vec<Key> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, repeat: false, .. } => Some(*key),
                    _ => None,
                })
                .collect()
        });
        let Some(&key) = presses.first() else { return };
        let dir = sounds_dir();

        if let Some(rel) = self.capture.clone() {
            self.capture = None;
            match key {
                Key::Escape => {}
                Key::Backspace | Key::Delete => {
                    self.hotkeys.remove(&rel);
                    save_hotkeys(&dir, &self.hotkeys);
                    self.sound_status = "Клавиша убрана".to_string();
                }
                k => {
                    let name = k.name().to_string();
                    let had_other = self.hotkeys.iter().any(|(r, v)| *v == name && *r != rel);
                    self.hotkeys.retain(|r, v| *v != name || *r == rel);
                    self.hotkeys.insert(rel, name.clone());
                    save_hotkeys(&dir, &self.hotkeys);
                    self.sound_status = if had_other {
                        format!("Клавиша {} назначена; у другого звука она снята", key_label(&name))
                    } else {
                        format!("Клавиша {} назначена", key_label(&name))
                    };
                }
            }
            return;
        }

        if ctx.wants_keyboard_input() {
            return;
        }
        for key in presses {
            let name = key.name();
            let rel = self.hotkeys.iter().find(|(_, v)| v.as_str() == name).map(|(r, _)| r.clone());
            if let Some(rel) = rel {
                self.sound_toggle(&rel);
            }
        }
    }

    fn sounds_ui(&mut self, ui: &mut egui::Ui) {
        if !self.sounds_loaded {
            self.sounds_reload();
        }
        let ctx = ui.ctx().clone();

        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.sounds_add(dropped);
        }

        header(
            ui,
            "Саундпад",
            "Нажмите на плитку или назначенную клавишу, чтобы проиграть звук. Правая кнопка мыши открывает меню звука.",
        );

        // Панель управления.
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if accent_button(ui, "+ Звуки").clicked() {
                    let picked = rfd::FileDialog::new()
                        .set_title("Выберите звуки")
                        .add_filter("Аудио", &AUDIO_EXTS)
                        .pick_files();
                    if let Some(files) = picked {
                        self.sounds_add(files);
                    }
                }
                if pill_button(ui, "+ Папка целиком").clicked() {
                    if let Some(folder) = rfd::FileDialog::new().set_title("Выберите папку со звуками").pick_folder() {
                        self.sounds_add(vec![folder]);
                    }
                }
                if pill_button(ui, "Стоп").clicked() {
                    self.sound_stop_all();
                }
                if pill_button(ui, "Открыть папку").clicked() {
                    open_folder(&sounds_dir());
                }
                if pill_button(ui, "Обновить").clicked() {
                    self.sounds_reload();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Поиск");
                ui.add(
                    egui::TextEdit::singleline(&mut self.sound_search)
                        .desired_width(190.0)
                        .hint_text("название звука"),
                );
                ui.add_space(10.0);
                ui.label("Громкость");
                let changed = ui.add(egui::Slider::new(&mut self.cfg.volume, 0.0..=1.5).show_value(false)).changed();
                if changed {
                    self.sound_set_volumes();
                }
                ui.label(RichText::new(format!("{:.0}%", self.cfg.volume * 100.0)).color(MUTED));
            });
            ui.checkbox(&mut self.cfg.overlap, "Накладывать звуки друг на друга");
            if !self.sound_status.is_empty() {
                ui.label(RichText::new(&self.sound_status).size(13.0).color(MUTED));
            }
        });

        self.audio_ui(ui);

        // Группы.
        let mut new_sel: Option<Option<String>> = None;
        let mut del_group: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            let total = self.sounds.len();
            if ui.selectable_label(self.group_sel.is_none(), format!("Все ({total})")).clicked() {
                new_sel = Some(None);
            }
            if !self.groups.is_empty() {
                let loose = self.sounds.iter().filter(|s| s.group.is_empty()).count();
                let selected = self.group_sel.as_deref() == Some("");
                if ui.selectable_label(selected, format!("Без группы ({loose})")).clicked() {
                    new_sel = Some(Some(String::new()));
                }
            }
            for g in &self.groups {
                let count = self.sounds.iter().filter(|s| s.group == *g).count();
                let selected = self.group_sel.as_deref() == Some(g.as_str());
                let r = ui.selectable_label(selected, format!("{g} ({count})"));
                if r.clicked() {
                    new_sel = Some(Some(g.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button("Удалить группу (если пустая)").clicked() {
                        del_group = Some(g.clone());
                        ui.close_menu();
                    }
                });
            }
            if pill_button(ui, "+ Группа").clicked() {
                self.new_group = Some(String::new());
            }
        });
        if let Some(s) = new_sel {
            self.group_sel = s;
        }
        if let Some(g) = del_group {
            self.group_delete(&g);
        }
        ui.add_space(6.0);

        // Новая группа.
        if let Some(mut name) = self.new_group.take() {
            let (mut keep, mut create) = (true, false);
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut name).desired_width(260.0).hint_text("Название новой группы"),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if accent_button(ui, "Создать").clicked() || enter {
                        create = true;
                    }
                    if pill_button(ui, "Отмена").clicked() {
                        keep = false;
                    }
                });
            });
            if create {
                match create_group(&sounds_dir(), &name) {
                    Ok(g) => {
                        self.group_sel = Some(g.clone());
                        self.sounds_reload();
                        self.sound_status = format!("Группа «{g}» создана. Новые звуки попадут в неё.");
                    }
                    Err(e) => {
                        self.sound_status = e;
                        self.new_group = Some(name);
                    }
                }
            } else if keep {
                self.new_group = Some(name);
            }
        }

        // Переименование.
        if let Some((rel, mut text, first)) = self.rename.take() {
            let (mut save, mut cancel) = (false, false);
            card(ui, |ui| {
                ui.label(RichText::new("Новое название звука").size(13.0).color(link()));
                ui.horizontal(|ui| {
                    let resp = ui.add(egui::TextEdit::singleline(&mut text).desired_width(300.0));
                    if first {
                        resp.request_focus();
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if accent_button(ui, "Сохранить").clicked() || enter {
                        save = true;
                    }
                    if pill_button(ui, "Отмена").clicked() {
                        cancel = true;
                    }
                });
            });
            if save {
                self.sound_rename(&rel, &text);
            } else if !cancel {
                self.rename = Some((rel, text, false));
            }
        }

        // Назначение клавиши.
        if let Some(rel) = self.capture.clone() {
            let name = self.sounds.iter().find(|s| s.rel == rel).map(|s| s.name.clone()).unwrap_or_default();
            card(ui, |ui| {
                ui.label(
                    RichText::new(format!(
                        "Нажмите клавишу для «{name}». Esc: отмена. Backspace: убрать клавишу."
                    ))
                    .color(YELLOW),
                );
            });
        }

        // Подтверждение удаления.
        if let Some(rel) = self.confirm_delete.clone() {
            let name = self.sounds.iter().find(|s| s.rel == rel).map(|s| s.name.clone()).unwrap_or_default();
            card(ui, |ui| {
                ui.label(
                    RichText::new(format!("Удалить «{name}»? Файл исчезнет и из папки sounds.")).color(RED),
                );
                ui.horizontal(|ui| {
                    if pill_button(ui, "Да, удалить").clicked() {
                        self.sound_delete(&rel);
                    }
                    if pill_button(ui, "Отмена").clicked() {
                        self.confirm_delete = None;
                    }
                });
            });
        }

        if self.sounds.is_empty() {
            ui.label(
                RichText::new(
                    "Пока нет ни одного звука. Нажмите «+ Звуки» или перетащите файлы (и целые папки) в окно.",
                )
                .color(MUTED),
            );
            return;
        }

        // Плитки.
        let query = self.sound_search.trim().to_lowercase();
        let visible: Vec<usize> = self
            .sounds
            .iter()
            .enumerate()
            .filter(|(_, s)| self.group_sel.as_ref().map(|g| s.group == *g).unwrap_or(true))
            .filter(|(_, s)| query.is_empty() || s.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        if visible.is_empty() {
            ui.label(RichText::new("Ничего не найдено.").color(MUTED));
            return;
        }

        let mut act: Option<(String, TileAction)> = None;
        let groups = self.groups.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(10.0);
            for &i in &visible {
                let s = &self.sounds[i];
                let playing = self.players.iter().find(|p| p.rel == s.rel);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(176.0, 68.0), Sense::click());

                let fill = if playing.is_some() {
                    Color32::from_rgb(48, 80, 140)
                } else if resp.hovered() {
                    Color32::from_rgb(63, 64, 68)
                } else {
                    FIELD_BG
                };
                let painter = ui.painter().clone();
                painter.rect_filled(rect, CornerRadius::same(14), fill);
                let bar = egui::Rect::from_min_size(rect.left_top() + Vec2::new(0.0, 12.0), Vec2::new(4.0, rect.height() - 24.0));
                painter.rect_filled(bar, CornerRadius::same(2), group_color(&s.group));

                let mut job = egui::text::LayoutJob::simple(
                    s.name.clone(),
                    FontId::proportional(14.0),
                    TEXT,
                    rect.width() - 44.0,
                );
                job.wrap.max_rows = 2;
                job.wrap.break_anywhere = true;
                let galley = painter.layout_job(job);
                let text_y = rect.top() + 8.0 + (40.0 - galley.size().y) / 2.0;
                painter.galley(egui::pos2(rect.left() + 16.0, text_y), galley, TEXT);

                if let Some(k) = self.hotkeys.get(&s.rel) {
                    painter.text(
                        rect.left_bottom() + Vec2::new(16.0, -9.0),
                        Align2::LEFT_BOTTOM,
                        format!("клавиша {}", key_label(k)),
                        FontId::proportional(11.0),
                        MUTED,
                    );
                }
                if let Some(p) = playing {
                    let frac = match p.total {
                        Some(t) if t.as_secs_f32() > 0.0 => (p.pos().as_secs_f32() / t.as_secs_f32()).clamp(0.0, 1.0),
                        _ => 0.0,
                    };
                    let track = egui::Rect::from_min_max(
                        egui::pos2(rect.left() + 16.0, rect.bottom() - 8.0),
                        egui::pos2(rect.right() - 16.0, rect.bottom() - 5.0),
                    );
                    painter.rect_filled(track, CornerRadius::same(2), Color32::from_rgb(31, 31, 31));
                    let done = egui::Rect::from_min_size(track.min, Vec2::new(track.width() * frac, track.height()));
                    painter.rect_filled(done, CornerRadius::same(2), link());
                    // Значок «играет»: маленький треугольник.
                    let o = egui::pos2(rect.right() - 20.0, rect.top() + 10.0);
                    painter.add(egui::Shape::convex_polygon(
                        vec![o, o + Vec2::new(0.0, 12.0), o + Vec2::new(10.0, 6.0)],
                        link(),
                        Stroke::NONE,
                    ));
                }

                resp.context_menu(|ui| {
                    if ui.button("Играть или стоп").clicked() {
                        act = Some((s.rel.clone(), TileAction::Toggle));
                        ui.close_menu();
                    }
                    if ui.button("Переименовать").clicked() {
                        act = Some((s.rel.clone(), TileAction::Rename));
                        ui.close_menu();
                    }
                    if ui.button("Назначить клавишу").clicked() {
                        act = Some((s.rel.clone(), TileAction::Hotkey));
                        ui.close_menu();
                    }
                    ui.menu_button("Переместить в группу", |ui| {
                        if !s.group.is_empty() && ui.button("Без группы").clicked() {
                            act = Some((s.rel.clone(), TileAction::Move(String::new())));
                            ui.close_menu();
                        }
                        for g in &groups {
                            if *g != s.group && ui.button(g).clicked() {
                                act = Some((s.rel.clone(), TileAction::Move(g.clone())));
                                ui.close_menu();
                            }
                        }
                        if groups.is_empty() {
                            ui.label(RichText::new("Сначала создайте группу").color(MUTED));
                        }
                    });
                    ui.separator();
                    if ui.button("Удалить").clicked() {
                        act = Some((s.rel.clone(), TileAction::Delete));
                        ui.close_menu();
                    }
                });
                if resp.clicked() {
                    act = Some((s.rel.clone(), TileAction::Toggle));
                }
                resp.on_hover_cursor(CursorIcon::PointingHand);
            }
        });

        if let Some((rel, a)) = act {
            self.sound_action(rel, a);
        }
    }

    // ── случайные числа и колесо удачи ──

    fn random_ui(&mut self, ui: &mut egui::Ui) {
        header(ui, "Случайные числа", "Кубик, монетка и число из любого диапазона.");
        self.numbers_ui(ui);
    }

    fn wheel_window_ui(&mut self, ui: &mut egui::Ui) {
        header(ui, "Колесо удачи", "Впишите варианты по одному в строке и крутите: каждый выпадает с равным шансом.");
        self.wheel_ui(ui);
    }

    fn numbers_ui(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if accent_button(ui, "Бросить кубик").clicked() {
                    self.rnd_die = self.rng.range(1, 6) as u8;
                    self.rnd_coin = 0;
                    self.rnd_result.clear();
                    self.rnd_pop = Instant::now();
                }
                if accent_button(ui, "Подбросить монетку").clicked() {
                    self.rnd_coin = if self.rng.below(2) == 0 { 1 } else { 2 };
                    self.rnd_die = 0;
                    self.rnd_result.clear();
                    self.rnd_pop = Instant::now();
                }
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("От").color(MUTED));
                ui.add(egui::DragValue::new(&mut self.rnd_min).range(-1_000_000_000_000i64..=1_000_000_000_000i64));
                ui.label(RichText::new("до").color(MUTED));
                ui.add(egui::DragValue::new(&mut self.rnd_max).range(-1_000_000_000_000i64..=1_000_000_000_000i64));
                if pill_button(ui, "Случайное число").clicked() {
                    let n = self.rng.range(self.rnd_min, self.rnd_max);
                    self.rnd_result = format!("{n}");
                    self.rnd_die = 0;
                    self.rnd_coin = 0;
                    self.rnd_pop = Instant::now();
                }
            });
            ui.add_space(10.0);
            // Результат «выскакивает»: быстро вырастает с небольшим перелётом и оседает.
            let k = (self.rnd_pop.elapsed().as_secs_f32() / 0.45).min(1.0);
            let scale = if k < 1.0 { 1.0 - (1.0 - k).powi(3) * (k * 9.0).cos().abs() + 0.12 * (1.0 - k) } else { 1.0 };
            if k < 1.0 {
                ui.ctx().request_repaint();
            }
            ui.vertical_centered(|ui| {
                if self.rnd_die > 0 {
                    draw_die(ui, self.rnd_die, 110.0 * scale.max(0.05), 130.0);
                } else if self.rnd_coin > 0 {
                    draw_coin(ui, self.rnd_coin == 1, 110.0 * scale.max(0.05), 130.0);
                } else if !self.rnd_result.is_empty() {
                    ui.add_space(14.0);
                    ui.label(RichText::new(&self.rnd_result).size((54.0 * scale).max(8.0)).strong().color(link()));
                    ui.add_space(14.0);
                } else {
                    ui.add_space(30.0);
                    ui.label(RichText::new("Нажмите кнопку, и выпадет результат").size(14.0).color(MUTED));
                    ui.add_space(30.0);
                }
            });
        });
    }

    fn wheel_save(&self) {
        let _ = std::fs::write(WHEEL_FILE, &self.wheel_text);
    }

    fn wheel_spin_start(&mut self) {
        let items = wheel_items(&self.wheel_text);
        match items.len() {
            0 => {}
            1 => self.wheel_finish(items, 0),
            _ => {
                // Итоговый угол равномерно случайный, поэтому каждый вариант выпадает с равным шансом.
                let turns = 5.0 + 3.0 * self.rng.unit();
                let extra = TAU * self.rng.unit();
                self.wheel_spin = Some(Spin {
                    start: Instant::now(),
                    duration: 4.5 + 1.5 * self.rng.unit(),
                    from: self.wheel_angle,
                    delta: TAU * turns + extra,
                });
            }
        }
    }

    fn wheel_finish(&mut self, items: Vec<String>, winner: usize) {
        self.wheel_result = items[winner].clone();
        self.wheel_history.insert(0, items[winner].clone());
        self.wheel_history.truncate(8);
        if self.wheel_remove {
            let rest: Vec<&str> =
                items.iter().enumerate().filter(|(i, _)| *i != winner).map(|(_, s)| s.as_str()).collect();
            self.wheel_text = rest.join("\n");
            self.wheel_save();
        }
    }

    fn wheel_ui(&mut self, ui: &mut egui::Ui) {
        // Анимация вращения: плавное замедление к концу.
        let mut finished = false;
        if let Some(sp) = &self.wheel_spin {
            let t = (sp.start.elapsed().as_secs_f64() / sp.duration).min(1.0);
            let eased = 1.0 - (1.0 - t).powi(3);
            self.wheel_angle = sp.from + sp.delta * eased;
            if t >= 1.0 {
                finished = true;
            } else {
                ui.ctx().request_repaint();
            }
        }
        if finished {
            self.wheel_spin = None;
            self.wheel_angle = self.wheel_angle.rem_euclid(TAU);
            let items = wheel_items(&self.wheel_text);
            if !items.is_empty() {
                let w = wheel_winner(self.wheel_angle, items.len());
                self.wheel_finish(items, w);
            }
        }

        let items = wheel_items(&self.wheel_text);
        let spinning = self.wheel_spin.is_some();
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("wheel_spinning"), spinning));
        let mut spin_clicked = false;

        card(ui, |ui| {
            let wide = ui.available_width() > 640.0;
            let wheel_size = if wide { 340.0 } else { (ui.available_width() - 8.0).clamp(200.0, 340.0) };
            let layout = if wide {
                egui::Layout::left_to_right(egui::Align::Min)
            } else {
                egui::Layout::top_down(egui::Align::Min)
            };
            ui.with_layout(layout, |ui| {
                if draw_wheel(ui, &items, self.wheel_angle, wheel_size) {
                    spin_clicked = true;
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new("Варианты, по одному в строке").size(13.0).color(link()));
                    let resp = ui.add_enabled(
                        !spinning,
                        egui::TextEdit::multiline(&mut self.wheel_text)
                            .desired_rows(8)
                            .desired_width(if wide { 250.0 } else { ui.available_width() })
                            .hint_text("Пицца\nСуши\nБургер"),
                    );
                    if resp.changed() {
                        self.wheel_save();
                    }
                    ui.add_enabled_ui(!spinning, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if accent_button(ui, "Крутить").clicked() {
                                spin_clicked = true;
                            }
                            if pill_button(ui, "Перемешать").clicked() {
                                let mut v = wheel_items(&self.wheel_text);
                                for i in (1..v.len()).rev() {
                                    let j = self.rng.below(i as u64 + 1) as usize;
                                    v.swap(i, j);
                                }
                                self.wheel_text = v.join("\n");
                                self.wheel_save();
                            }
                            if pill_button(ui, "Очистить").clicked() {
                                self.wheel_text.clear();
                                self.wheel_save();
                            }
                        });
                        if ui.checkbox(&mut self.wheel_remove, "Убирать выпавший вариант").changed() {
                            self.wheel_save();
                        }
                    });
                    ui.label(RichText::new(format!("Вариантов: {}", items.len())).size(12.0).color(MUTED));
                });
            });
        });
        if spin_clicked && !spinning {
            self.wheel_spin_start();
        }

        if !self.wheel_result.is_empty() && !spinning {
            card(ui, |ui| {
                ui.label(RichText::new("Выпало").size(13.0).color(link()));
                ui.label(RichText::new(&self.wheel_result).size(34.0).color(TEXT));
                if self.wheel_history.len() > 1 {
                    ui.label(
                        RichText::new(format!("Раньше: {}", self.wheel_history[1..].join(", ")))
                            .size(13.0)
                            .color(MUTED),
                    );
                }
            });
        }
    }

    // ── калькулятор ──

    fn calc_run(&mut self) {
        if self.calc_input.trim().is_empty() {
            return;
        }
        match eval(&self.calc_input) {
            Ok(v) => {
                let result = fmt_num(v);
                self.calc_history.insert(0, format!("{} = {}", self.calc_input.trim(), result));
                self.calc_history.truncate(10);
                self.calc_input = result;
                self.calc_error.clear();
            }
            Err(e) => self.calc_error = e,
        }
    }

    fn calc_ui(&mut self, ui: &mut egui::Ui) {
        header(ui, "Калькулятор", "Скобки, ^ (степень), sqrt, sin, cos, tan, ln, log, abs, pi, e. Дробь можно писать и через запятую.");
        card(ui, |ui| {
            let resp = Frame::new()
                .fill(Color32::from_rgb(20, 20, 29))
                .corner_radius(CornerRadius::same(16))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(160, 150, 255, 40)))
                .inner_margin(Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.calc_input)
                            .frame(false)
                            .horizontal_align(egui::Align::RIGHT)
                            .desired_width(f32::INFINITY)
                            .font(FontId::proportional(30.0))
                            .hint_text("(2+3)*4^2"),
                    )
                })
                .inner;
            ui.add_space(6.0);
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.calc_run();
                resp.request_focus();
            }
            if !self.calc_error.is_empty() {
                ui.label(RichText::new(&self.calc_error).color(RED));
            }

            let mut press: Option<&'static str> = None;
            let key_w = ((ui.available_width() - 4.0 * 8.0) / 5.0).clamp(44.0, 78.0);
            egui::Grid::new("keypad").spacing([8.0, 8.0]).show(ui, |ui| {
                for row in KEYS.iter() {
                    for k in row.iter() {
                        if calc_key(ui, k, key_w).clicked() {
                            press = Some(*k);
                        }
                    }
                    ui.end_row();
                }
            });
            if let Some(k) = press {
                match k {
                    "C" => {
                        self.calc_input.clear();
                        self.calc_error.clear();
                    }
                    "Стереть" => {
                        self.calc_input.pop();
                    }
                    "=" => self.calc_run(),
                    "sqrt" | "ln" => self.calc_input.push_str(&format!("{k}(")),
                    _ => self.calc_input.push_str(k),
                }
            }
        });

        if !self.calc_history.is_empty() {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("История").size(13.0).color(link()));
                    if pill_button(ui, "Очистить").clicked() {
                        self.calc_history.clear();
                    }
                });
                for line in &self.calc_history {
                    ui.label(RichText::new(line).size(15.0).color(MUTED));
                }
            });
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        sync_live(&self.bcfg);
        if let Some(rx) = &self.ext_rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.ext_status = match r {
                        Ok(name) => format!("Установлено: {name}. Перезапустите Petal, чтобы расширение заработало."),
                        Err(e) => e,
                    };
                    self.ext_rx = None;
                    self.ext_list = list_extensions();
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(250)),
                Err(_) => self.ext_rx = None,
            }
        }
        let key = pages_key(&self.bcfg);
        if key != self.applied_search {
            write_pages(&self.bcfg);
            self.applied_search = key;
        }
        let (ca, cb) = store::theme_colors(&self.bcfg);
        ACC_A.store(ca, Ordering::Relaxed);
        ACC_B.store(cb, Ordering::Relaxed);
        ANIM_ON.store(self.bcfg.anim, Ordering::Relaxed);
        if self.applied_scale != self.bcfg.ui_scale {
            ctx.set_zoom_factor(self.bcfg.ui_scale as f32 / 100.0);
            self.applied_scale = self.bcfg.ui_scale;
        }
        if self.bcfg.zoom != self.applied_zoom {
            for t in &self.tabs {
                if let Some(b) = &t.view {
                    b.set_zoom(self.bcfg.zoom);
                }
            }
            self.applied_zoom = self.bcfg.zoom;
        }
        self.drain_new_tabs();
        self.drain_downloads();
        self.audio_sync();
        self.players.retain(|p| !p.empty());
        if !self.players.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.sound_keys(ctx);

        self.update_page_fullscreen(ctx);
        self.update_mini_player(ctx);

        if !self.page_fs && self.mini.is_none() {
            self.tabs_panel(ctx);
            self.menus(ctx);
        }
        self.tool_windows(ctx);

        let active = self.active;
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if i != active {
                t.park();
            }
        }
        let page_fill = page_bg(&self.tabs[active].current_url(), self.tabs[active].incognito);
        egui::CentralPanel::default().frame(Frame::new().fill(page_fill)).show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            ui.allocate_rect(rect, Sense::hover());
            self.tabs[active].show(frame, ui, rect);
        });

        // Адрес и названия вкладок меняются сами при переходах.
        if !self.addr_focused {
            if let Some(u) = self.tabs[active].url() {
                if !u.is_empty() && u != self.addr {
                    self.addr = shown_url(&u);
                }
            }
        }
        if self.tabs.iter().any(|t| t.view.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        self.pane_tick(ctx);
        self.poll_updates(ctx);
        self.persist_tick();
        if self.cfg != self.saved_cfg {
            save_settings(&self.cfg);
            self.saved_cfg = self.cfg.clone();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.bcfg.wipe_incognito {
            for t in &self.tabs {
                if t.incognito {
                    if let Some(v) = &t.view {
                        v.clear_data();
                    }
                }
            }
        }
        self.history_tick();
        self.flush(true);
        self.vault_lock();
    }
}

// ───────────────────────── Логотип «Petal» ─────────────────────────

/// Лепестки цветка: (смещение по углу в градусах, цвет). Один и тот же рисунок идёт и в окно, и в иконку.
const PETALS: [(f32, [u8; 3]); 5] = [
    (-90.0, [236, 72, 110]),
    (-18.0, [250, 130, 90]),
    (54.0, [255, 190, 90]),
    (126.0, [200, 80, 170]),
    (198.0, [150, 110, 240]),
];

/// Рисует логотип в квадрате `r`: тёмная плитка и пять перекрывающихся лепестков вокруг сердцевины.
fn paint_logo(p: &egui::Painter, r: egui::Rect) {
    let s = r.width();
    let c = r.center();
    p.rect_filled(r, CornerRadius::same((s * 0.24) as u8), Color32::from_rgb(34, 34, 42));
    for (deg, col) in PETALS {
        let a = deg.to_radians();
        let pos = c + Vec2::new(a.cos(), a.sin()) * (s * 0.2);
        p.circle_filled(pos, s * 0.22, Color32::from_rgba_unmultiplied(col[0], col[1], col[2], 235));
    }
    p.circle_filled(c, s * 0.1, Color32::from_rgb(34, 34, 42));
}

/// Та же картинка, нарисованная по пикселям, для значка окна и панели задач.
fn logo_icon() -> egui::IconData {
    const N: u32 = 64;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    let s = N as f32;
    let c = s / 2.0;
    let bg = [34u8, 34, 42];
    for y in 0..N {
        for x in 0..N {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // скруглённая плитка
            let rad = s * 0.24;
            let dx = (px - c).abs() - (c - rad);
            let dy = (py - c).abs() - (c - rad);
            let inside = if dx > 0.0 && dy > 0.0 { dx * dx + dy * dy <= rad * rad } else { true };
            if !inside {
                continue;
            }
            let mut col = bg;
            for (deg, pc) in PETALS {
                let a = deg.to_radians();
                let (qx, qy) = (c + a.cos() * s * 0.2, c + a.sin() * s * 0.2);
                if (px - qx).powi(2) + (py - qy).powi(2) <= (s * 0.22).powi(2) {
                    col = pc;
                }
            }
            if (px - c).powi(2) + (py - c).powi(2) <= (s * 0.1).powi(2) {
                col = bg;
            }
            let i = ((y * N + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[col[0], col[1], col[2], 255]);
        }
    }
    egui::IconData { rgba, width: N, height: N }
}

/// Установленная программа лежит в Program Files, куда писать нельзя, поэтому все файлы данных
/// (вкладки, история, звуки, профиль браузера) переезжают в папку пользователя.
fn setup_data_dir() {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let portable = exe_dir.as_ref().is_some_and(|d| d.join("portable.txt").exists());
    // На других системах работаем в текущей папке, как при разработке.
    let base = if cfg!(windows) { std::env::var_os("LOCALAPPDATA") } else { None };
    let Some(dir) = store::pick_data_dir(base, exe_dir, portable && cfg!(windows)) else { return };
    if std::fs::create_dir_all(&dir).is_ok() && std::env::set_current_dir(&dir).is_ok() {
        // Браузер WebView2 по умолчанию пишет рядом с программой, то есть тоже в Program Files.
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dir.join("WebView2"));
    }
}

fn main() -> eframe::Result<()> {
    setup_data_dir();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("Petal")
            .with_icon(logo_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Petal",
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}


// ───────────────────────── Тесты ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_icon_is_valid() {
        let i = logo_icon();
        assert_eq!(i.rgba.len(), (i.width * i.height * 4) as usize);
        // углы прозрачные, центр плитки непрозрачный
        assert_eq!(i.rgba[3], 0);
        let mid = ((32 * 64 + 32) * 4 + 3) as usize;
        assert_eq!(i.rgba[mid], 255);
    }

    #[test]
    fn arithmetic() {
        assert_eq!(eval("2+2*3").unwrap(), 8.0);
        assert_eq!(eval("(1+2)^2").unwrap(), 9.0);
        assert_eq!(eval("-2^2").unwrap(), -4.0);
        assert_eq!(eval("2^-1").unwrap(), 0.5);
        assert_eq!(eval("2,5*2").unwrap(), 5.0);
        assert_eq!(eval("sqrt(81)").unwrap(), 9.0);
    }

    #[test]
    fn errors() {
        assert!(eval("1/0").is_err());
        assert!(eval("2+").is_err());
        assert!(eval("(1+2").is_err());
        assert!(eval("sqrt(-1)").is_err());
        assert!(eval("abc").is_err());
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_num(8.0), "8");
        assert_eq!(fmt_num(0.1 + 0.2), "0.3");
    }

    #[test]
    fn rng_range_and_unit() {
        let mut r = Rng::new();
        for _ in 0..1000 {
            let n = r.range(1, 6);
            assert!((1..=6).contains(&n));
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
        }
    }

    #[test]
    fn url_normalizing() {
        assert_eq!(normalize_url("https://example.com/a"), "https://example.com/a");
        assert_eq!(normalize_url("  yandex.ru "), "https://yandex.ru");
        assert_eq!(
            normalize_url("рецепт борща"),
            "https://www.google.com/search?q=%D1%80%D0%B5%D1%86%D0%B5%D0%BF%D1%82+%D0%B1%D0%BE%D1%80%D1%89%D0%B0"
        );
        assert_eq!(normalize_url("rust lang.org"), "https://www.google.com/search?q=rust+lang.org");
    }

    // ── колесо ──

    #[test]
    fn wheel_items_trim_and_skip_empty() {
        assert_eq!(wheel_items("  Пицца \n\n Суши\n   \nБургер"), vec!["Пицца", "Суши", "Бургер"]);
        assert!(wheel_items("  \n \n").is_empty());
    }

    #[test]
    fn wheel_winner_matches_geometry() {
        // Без поворота под стрелкой (направление вверх, 270°) оказывается последний из четырёх секторов.
        assert_eq!(wheel_winner(0.0, 4), 3);
        // Повернули на четверть оборота по часовой стрелке: под стрелку приехал предыдущий сектор.
        assert_eq!(wheel_winner(std::f64::consts::FRAC_PI_2, 4), 2);
        // Полный оборот ничего не меняет.
        assert_eq!(wheel_winner(TAU + 0.3, 5), wheel_winner(0.3, 5));
        // Любой угол даёт допустимый индекс.
        for k in 0..1000 {
            let a = k as f64 * 0.0173 - 5.0;
            assert!(wheel_winner(a, 7) < 7);
        }
    }

    #[test]
    fn wheel_is_fair() {
        // Равномерно «прокручиваем» угол: каждый из 5 вариантов должен получить ровно пятую часть.
        let n = 5;
        let steps = 100_000;
        let mut counts = vec![0usize; n];
        for k in 0..steps {
            counts[wheel_winner(TAU * k as f64 / steps as f64, n)] += 1;
        }
        for c in counts {
            assert!((c as i64 - (steps / n) as i64).abs() <= 1, "перекос: {c}");
        }
    }

    // ── саундпад ──

    fn temp_dir(tag: &str) -> PathBuf {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("helper_test_{tag}_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Минимальный настоящий WAV: 16 бит, моно, 8 кГц, 800 отсчётов тишины.
    fn wav_bytes() -> Vec<u8> {
        let data_len: u32 = 800 * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        v.extend(std::iter::repeat_n(0u8, data_len as usize));
        v
    }

    #[test]
    fn sound_helpers() {
        assert!(is_audio_file(Path::new("a/b/Звук.MP3")));
        assert!(is_audio_file(Path::new("x.flac")));
        assert!(!is_audio_file(Path::new("x.txt")));
        assert!(!is_audio_file(Path::new("noext")));
        assert_eq!(sound_name(Path::new("dir/мой звук.ogg")), "мой звук");
        assert!(list_sounds(Path::new("/такой/папки/нет")).is_empty());
        assert_eq!(clean_name("  а/б:в*?  "), "а_б_в__");
        assert_eq!(clean_name("имя..."), "имя");
        assert_eq!(clean_name("   "), "");
        assert_eq!(key_label("Num5"), "5");
        assert_eq!(key_label("Q"), "Q");
    }

    #[test]
    fn sounds_import_and_list() {
        let src = temp_dir("src");
        let dst = temp_dir("dst").join("sounds"); // папки ещё нет, import_sound её создаст

        let good = src.join("Привет.wav");
        std::fs::write(&good, wav_bytes()).unwrap();
        let broken = src.join("битый.mp3");
        std::fs::write(&broken, "это не аудио").unwrap();
        let text = src.join("заметки.txt");
        std::fs::write(&text, b"hello").unwrap();

        assert_eq!(import_sound(&good, &dst, "").unwrap(), "Привет");
        assert_eq!(import_sound(&good, &dst, "").unwrap(), "Привет (2)");
        assert_eq!(import_sound(&good, &dst, "Мемы").unwrap(), "Привет");
        assert!(import_sound(&broken, &dst, "").unwrap_err().contains("повреждён"));
        assert!(import_sound(&text, &dst, "").unwrap_err().contains("неподдерживаемый"));
        assert!(import_sound(&src.join("нет.wav"), &dst, "").is_err());

        let all = list_sounds(&dst);
        let rels: Vec<&str> = all.iter().map(|s| s.rel.as_str()).collect();
        assert_eq!(rels, vec!["Привет.wav", "Привет (2).wav", "Мемы/Привет.wav"]);
        assert_eq!(all[2].group, "Мемы");
        assert_eq!(list_groups(&dst), vec!["Мемы"]);

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(dst.parent().unwrap());
    }

    #[test]
    fn sounds_folder_import_makes_group() {
        let src = temp_dir("folder").join("Мои звуки");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.wav"), wav_bytes()).unwrap();
        std::fs::write(src.join("b.wav"), wav_bytes()).unwrap();
        std::fs::write(src.join("c.mp3"), "не аудио").unwrap();
        std::fs::write(src.join("readme.txt"), "x").unwrap();
        let dst = temp_dir("folder_dst");

        let (ok, errors) = import_folder(&src, &dst);
        assert_eq!(ok, 2);
        assert_eq!(errors.len(), 1);
        assert_eq!(list_groups(&dst), vec!["Мои звуки"]);
        assert_eq!(list_sounds(&dst).len(), 2);

        let _ = std::fs::remove_dir_all(src.parent().unwrap());
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn sounds_rename_move_and_groups() {
        let dir = temp_dir("ops");
        let src = temp_dir("ops_src");
        std::fs::write(src.join("один.wav"), wav_bytes()).unwrap();
        std::fs::write(src.join("два.wav"), wav_bytes()).unwrap();
        import_sound(&src.join("один.wav"), &dir, "").unwrap();
        import_sound(&src.join("два.wav"), &dir, "").unwrap();

        // Переименование: допустимое, занятое, пустое.
        let p = rename_sound(&dir.join("один.wav"), "Первый").unwrap();
        assert_eq!(p, dir.join("Первый.wav"));
        assert!(rename_sound(&p, "два").unwrap_err().contains("уже есть"));
        assert!(rename_sound(&p, "  ").is_err());
        assert_eq!(rename_sound(&p, "Первый").unwrap(), p);

        // Группы и перемещение.
        assert_eq!(create_group(&dir, " Мемы ").unwrap(), "Мемы");
        assert!(create_group(&dir, "  ").is_err());
        let moved = move_sound(&p, &dir, "Мемы").unwrap();
        assert_eq!(moved, dir.join("Мемы").join("Первый.wav"));
        assert!(moved.exists() && !p.exists());
        assert_eq!(rel_path(&dir, &moved), "Мемы/Первый.wav");
        // Обратно из группы в корень.
        let back = move_sound(&moved, &dir, "").unwrap();
        assert_eq!(back, dir.join("Первый.wav"));
        // Если имя в целевой группе занято, добавится номер.
        std::fs::write(dir.join("Мемы").join("два.wav"), wav_bytes()).unwrap();
        let two = move_sound(&dir.join("два.wav"), &dir, "Мемы").unwrap();
        assert_eq!(two, dir.join("Мемы").join("два (2).wav"));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn hotkeys_roundtrip() {
        let dir = temp_dir("keys");
        assert!(load_hotkeys(&dir).is_empty());
        let mut m = HashMap::new();
        m.insert("Мемы/Привет.wav".to_string(), "Q".to_string());
        m.insert("два.wav".to_string(), "Num1".to_string());
        save_hotkeys(&dir, &m);
        assert_eq!(load_hotkeys(&dir), m);
        // Файл с клавишами не считается звуком.
        assert!(list_sounds(&dir).is_empty());
        for k in [egui::Key::Q, egui::Key::Num1, egui::Key::F5] {
            assert_eq!(egui::Key::from_name(k.name()), Some(k));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tab_titles() {
        assert_eq!(tab_label("  Моя страница ", "https://a.com/x"), "Моя страница");
        assert_eq!(tab_label("", "https://www.spotify.com/path?q=1"), "spotify.com");
        assert_eq!(tab_label("", "about:blank"), "Новая вкладка");
        assert_eq!(host_of("https://open.spotify.com/"), Some("open.spotify.com".to_string()));
    }

    #[test]
    fn settings_roundtrip_and_garbage() {
        let c = AudioCfg {
            mic_on: true,
            cable: "CABLE Input (VB-Audio Virtual Cable)".into(),
            hear_self: false,
            mic_volume: 0.5,
            pass_on: false,
            input: "Микрофон (USB)".into(),
            volume: 1.25,
            overlap: true,
        };
        assert_eq!(settings_from_text(&settings_to_text(&c)), c);
        let bad = settings_from_text("volume=abc\nmic_volume=999\nмусор\nmic_on=1");
        assert_eq!(bad.volume, 1.0);
        assert_eq!(bad.mic_volume, 1.5);
        assert!(bad.mic_on);
        assert_eq!(settings_from_text(""), AudioCfg::default());
    }

    #[test]
    fn cable_and_mic_picking() {
        let outs: Vec<String> = vec!["Динамики (Realtek)".into(), "CABLE Input (VB-Audio Virtual Cable)".into()];
        assert_eq!(pick_cable(&outs).as_deref(), Some("CABLE Input (VB-Audio Virtual Cable)"));
        assert_eq!(pick_cable(&outs[..1]), None);
        let ins: Vec<String> = vec!["CABLE Output (VB-Audio Virtual Cable)".into(), "Микрофон (Realtek)".into()];
        assert_eq!(pick_input(&ins).as_deref(), Some("Микрофон (Realtek)"));
        assert_eq!(pick_input(&ins[..1]), None);
    }

    #[test]
    fn mic_feed_gives_silence_when_empty() {
        let (tx, rx) = mpsc::sync_channel::<f32>(4);
        let stop = Arc::new(AtomicBool::new(false));
        let mut f = MicFeed { rx, channels: std::num::NonZero::new(1).unwrap(), rate: std::num::NonZero::new(48000).unwrap(), stop: stop.clone() };
        tx.send(0.5).unwrap();
        assert_eq!(f.next(), Some(0.5));
        assert_eq!(f.next(), Some(0.0));
        stop.store(true, Ordering::Relaxed);
        assert_eq!(f.next(), None);
    }
}
