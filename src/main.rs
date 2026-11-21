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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ───────────────────────── Цвета ─────────────────────────

const BG: Color32 = Color32::from_rgb(31, 31, 31);
const SIDE_BG: Color32 = Color32::from_rgb(24, 24, 24);
const FIELD_BG: Color32 = Color32::from_rgb(48, 49, 52);
const CARD_BG: Color32 = Color32::from_rgb(41, 42, 45);
const TEXT: Color32 = Color32::from_rgb(232, 234, 237);
const MUTED: Color32 = Color32::from_rgb(154, 160, 166);
const LINK: Color32 = Color32::from_rgb(138, 180, 248);
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
const WHEEL_PALETTE: [Color32; 6] = [LINK, RED, YELLOW, GREEN, PURPLE, ORANGE];

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

/// Рисует колесо со стрелкой. Возвращает true, если нажали на центр.
fn draw_wheel(ui: &mut egui::Ui, items: &[String], angle: f64, size: f32) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter_at(rect);
    let c = rect.center();
    let r = size / 2.0 - 18.0;
    let n = items.len();

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
            painter.add(egui::Shape::convex_polygon(pts, sector_color(i, n), Stroke::new(2.0_f32, BG)));
        }
    }

    // Подписи вдоль радиуса. Слева текст переворачиваем, чтобы он не шёл вверх ногами.
    if (1..=36).contains(&n) {
        let font = FontId::proportional(if n <= 8 { 16.0 } else if n <= 16 { 13.0 } else { 11.0 });
        let step = TAU / n as f64;
        for (i, text) in items.iter().enumerate() {
            let mid = angle + step * (i as f64 + 0.5);
            let (cos, sin) = (mid.cos() as f32, mid.sin() as f32);
            let dir = Vec2::new(cos, sin);
            let mut job = egui::text::LayoutJob::simple(text.clone(), font.clone(), BG, r * 0.7);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let galley = painter.layout_job(job);
            let (theta, start) = if cos >= 0.0 {
                (mid as f32, c + dir * (r * 0.24))
            } else {
                (mid as f32 + std::f32::consts::PI, c + dir * (r * 0.94))
            };
            let y_axis = Vec2::new(-theta.sin(), theta.cos());
            let pos = start - y_axis * (galley.size().y / 2.0);
            painter.add(egui::epaint::TextShape::new(pos, galley, BG).with_angle(theta));
        }
    }

    // Стрелка сверху.
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x - 13.0, rect.top() + 2.0),
            egui::pos2(c.x + 13.0, rect.top() + 2.0),
            egui::pos2(c.x, rect.top() + 32.0),
        ],
        TEXT,
        Stroke::new(2.0_f32, BG),
    ));

    // Кнопка в центре.
    let hub = egui::Rect::from_center_size(c, Vec2::splat(64.0));
    let resp = ui
        .interact(hub, ui.id().with("wheel_hub"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let hub_fill = if resp.hovered() { Color32::from_rgb(63, 64, 68) } else { BG };
    painter.circle(c, 32.0, hub_fill, Stroke::new(3.0_f32, TEXT));
    painter.text(c, Align2::CENTER_CENTER, "Крутить", FontId::proportional(13.0), TEXT);
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

const HOME_URL: &str = "https://www.google.com/";

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

/// Ссылку http(s) оставляет как есть, «домен.ру» дополняет до https, всё остальное ищет в Google.
fn normalize_url(input: &str) -> String {
    let t = input.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        return t.to_string();
    }
    if !t.contains("://") && !t.contains(' ') && t.contains('.') && !t.ends_with('.') && !t.starts_with('.') {
        return format!("https://{t}");
    }
    format!("https://www.google.com/search?q={}", url_encode(t))
}

/// Настоящий браузер (WebView2, то есть Chromium из Edge) внутри окна. Только Windows.
#[cfg(windows)]
mod web {
    use std::sync::mpsc::Sender;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use wry::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
    use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows};
    use super::{DlEvent, DL_EVENTS};

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
        ) -> Result<Self, String> {
            let webview = WebViewBuilder::new()
                // Страница сообщает, когда её элемент (например, видео на YouTube) уходит на весь экран.
                .with_initialization_script(FULLSCREEN_JS)
                .with_initialization_script(PIP_JS)
                .with_ipc_handler(move |req| match req.body().as_str() {
                    "petal:fs:1" => fullscreen.store(true, Ordering::Relaxed),
                    "petal:fs:0" => fullscreen.store(false, Ordering::Relaxed),
                    "petal:pip:1" => pip.store(true, Ordering::Relaxed),
                    "petal:pip:0" => pip.store(false, Ordering::Relaxed),
                    _ => {}
                })
                .with_incognito(incognito)
                // Тёмный фон до загрузки страницы и тёмная тема для сайтов, которые её поддерживают.
                .with_background_color((31, 31, 31, 255))
                .with_theme(wry::Theme::Dark)
                .with_download_started_handler(|url, path| {
                    if let Ok(mut q) = DL_EVENTS.lock() {
                        q.push(DlEvent::Started { url, path: path.clone() });
                    }
                    true
                })
                .with_download_completed_handler(|url, path, ok| {
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
                        NewWindowResponse::Allow
                    } else {
                        let _ = new_tabs.send(url);
                        NewWindowResponse::Deny
                    }
                })
                .build_as_child(frame)
                .map_err(|e| e.to_string())?;
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
        /// Стирает cookie, кэш и прочие данные сайтов (из аккаунтов придётся выйти).
        pub fn clear_data(&self) {
            let _ = self.webview.clear_all_browsing_data();
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
        pub fn clear_data(&self) {}
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

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = FIELD_BG;
    v.selection.bg_fill = Color32::from_rgb(48, 80, 140);
    v.selection.stroke = Stroke::new(1.0_f32, TEXT);
    let radius = CornerRadius::same(18);
    v.widgets.noninteractive.corner_radius = radius;
    v.widgets.inactive.corner_radius = radius;
    v.widgets.hovered.corner_radius = radius;
    v.widgets.active.corner_radius = radius;
    v.widgets.inactive.weak_bg_fill = FIELD_BG;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(63, 64, 68);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(78, 80, 86);
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(10.0, 10.0);
        s.spacing.button_padding = Vec2::new(14.0, 7.0);
    });
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(CARD_BG)
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    ui.add_space(8.0);
}

fn header(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.label(RichText::new(title).size(28.0).color(TEXT));
    ui.label(RichText::new(sub).size(14.0).color(MUTED));
    ui.add_space(12.0);
}

#[derive(Clone, Copy)]
enum NavIcon {
    Back,
    Forward,
    Reload,
}

/// Круглая кнопка со значком, нарисованным линиями (не зависит от шрифта).
fn icon_button(ui: &mut egui::Ui, icon: NavIcon, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
    let resp = if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    let fill = if enabled && resp.hovered() { Color32::from_rgb(70, 72, 78) } else { FIELD_BG };
    let color = if enabled { TEXT } else { Color32::from_gray(95) };
    let p = ui.painter();
    p.circle_filled(rect.center(), 17.0, fill);
    let c = rect.center();
    let st = Stroke::new(2.0_f32, color);
    match icon {
        NavIcon::Back | NavIcon::Forward => {
            let d = if matches!(icon, NavIcon::Back) { -1.0 } else { 1.0 };
            p.line_segment([c + Vec2::new(-6.0 * d, 0.0), c + Vec2::new(6.0 * d, 0.0)], st);
            p.line_segment([c + Vec2::new(6.0 * d, 0.0), c + Vec2::new(1.0 * d, -5.0)], st);
            p.line_segment([c + Vec2::new(6.0 * d, 0.0), c + Vec2::new(1.0 * d, 5.0)], st);
        }
        NavIcon::Reload => {
            let r = 7.0_f32;
            let (a0, a1) = (-1.2_f32, -1.2_f32 + std::f32::consts::TAU * 0.78);
            let pts: Vec<egui::Pos2> = (0..=24)
                .map(|i| {
                    let a = a0 + (a1 - a0) * (i as f32 / 24.0);
                    c + Vec2::new(r * a.cos(), r * a.sin())
                })
                .collect();
            let end = *pts.last().unwrap_or(&c);
            p.add(egui::Shape::line(pts, st));
            // Наконечник стрелки: два штриха назад по касательной в конце дуги.
            let back = Vec2::new(a1.sin(), -a1.cos());
            for rot in [0.6_f32, -0.6] {
                let (sn, cs) = rot.sin_cos();
                let d = Vec2::new(back.x * cs - back.y * sn, back.x * sn + back.y * cs);
                p.line_segment([end, end + d * 5.5], st);
            }
        }
    }
    resp
}

/// Зелёная кнопка «Загрузки» внизу колонки вкладок: стрелка вниз над лотком и счётчик активных загрузок.
fn downloads_button(ui: &mut egui::Ui, active: usize) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let fill = if resp.hovered() { Color32::from_rgb(150, 215, 168) } else { GREEN };
    let dark = Color32::from_rgb(24, 46, 31);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), fill);
    let c = rect.left_center() + Vec2::new(24.0, 0.0);
    let st = Stroke::new(2.2_f32, dark);
    p.line_segment([c + Vec2::new(0.0, -8.0), c + Vec2::new(0.0, 3.0)], st);
    p.line_segment([c + Vec2::new(-5.0, -2.0), c + Vec2::new(0.0, 3.5)], st);
    p.line_segment([c + Vec2::new(5.0, -2.0), c + Vec2::new(0.0, 3.5)], st);
    p.line_segment([c + Vec2::new(-8.0, 8.0), c + Vec2::new(8.0, 8.0)], st);
    p.line_segment([c + Vec2::new(-8.0, 3.0), c + Vec2::new(-8.0, 8.0)], st);
    p.line_segment([c + Vec2::new(8.0, 3.0), c + Vec2::new(8.0, 8.0)], st);
    let label = if active > 0 { format!("Загрузки ({active})") } else { "Загрузки".to_string() };
    p.text(rect.left_center() + Vec2::new(46.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(15.0), dark);
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

/// Круглая кнопка с тремя точками (меню браузера).
fn dots_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let fill = if open || resp.hovered() { Color32::from_rgb(70, 72, 78) } else { FIELD_BG };
    let p = ui.painter();
    p.circle_filled(rect.center(), 17.0, fill);
    for dy in [-7.0_f32, 0.0, 7.0] {
        p.circle_filled(rect.center() + Vec2::new(0.0, dy), 2.2, TEXT);
    }
    resp
}

/// Пункт маленького меню на всю ширину; true, если нажали.
fn menu_item(ui: &mut egui::Ui, text: &str, color: Color32) -> bool {
    ui.add_sized([ui.available_width(), 34.0], egui::Button::new(RichText::new(text).size(15.0).color(color))).clicked()
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
    ui.add(egui::Button::new(RichText::new(text).color(TEXT)).min_size(Vec2::new(0.0, 34.0)))
        .on_hover_cursor(CursorIcon::PointingHand)
}

fn accent_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::from_rgb(32, 33, 36)))
            .fill(LINK)
            .min_size(Vec2::new(0.0, 34.0)),
    )
    .on_hover_cursor(CursorIcon::PointingHand)
}


/// Увядшая роза, нарисованная кистью egui (стикер).
fn paint_rose(p: &egui::Painter, r: egui::Rect) {
    use egui::epaint::CubicBezierShape;
    use egui::pos2;
    let u = r.width() / 42.0;
    let c = r.center();
    let at = |x: f32, y: f32| pos2(c.x + x * u, c.y + y * u);
    // Стебель, согнутый под тяжестью цветка.
    p.add(CubicBezierShape::from_points_stroke(
        [at(-4.0, 17.0), at(-5.0, 8.0), at(-3.0, 0.0), at(4.0, -3.0)],
        false,
        Color32::TRANSPARENT,
        Stroke::new(2.2 * u, Color32::from_rgb(76, 150, 92)),
    ));
    // Лист.
    p.add(egui::Shape::convex_polygon(
        vec![at(-4.6, 10.0), at(-12.0, 4.5), at(-9.0, 12.5)],
        Color32::from_rgb(76, 150, 92),
        Stroke::NONE,
    ));
    // Бутон: опущен вправо и вниз.
    let h = at(6.5, -2.5);
    p.circle_filled(h, 9.0 * u, Color32::from_rgb(176, 32, 48));
    p.circle_filled(h, 6.0 * u, Color32::from_rgb(138, 20, 38));
    let mut pts = Vec::new();
    for k in 0..40 {
        let a = k as f32 * 0.42;
        let rad = (0.6 + k as f32 * 0.115) * u;
        pts.push(pos2(h.x + rad * a.cos(), h.y + rad * a.sin()));
    }
    p.add(egui::Shape::line(pts, Stroke::new(1.2 * u, Color32::from_rgb(232, 92, 104))));
    p.circle_stroke(h, 9.0 * u, Stroke::new(1.0 * u, Color32::from_rgb(222, 70, 84)));
    // Упавший лепесток.
    p.circle_filled(at(10.0, 12.0), 2.3 * u, Color32::from_rgb(150, 24, 40));
    p.circle_filled(at(1.0, 15.0), 1.6 * u, Color32::from_rgb(176, 32, 48));
}

/// Кнопка-стикер справа сверху: открывает и закрывает панель функций.
fn sticker_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(42.0), Sense::click());
    let bg = if open {
        Color32::from_rgb(58, 64, 90)
    } else if resp.hovered() {
        Color32::from_rgb(52, 53, 57)
    } else {
        CARD_BG
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), Color32::from_rgb(238, 238, 238));
    let inner = rect.shrink(2.5);
    p.rect_filled(inner, CornerRadius::same(10), bg);
    paint_rose(p, inner);
    // Отклеивающийся уголок.
    let br = rect.right_bottom();
    p.add(egui::Shape::convex_polygon(
        vec![br, br - Vec2::new(11.0, 0.0), br - Vec2::new(0.0, 11.0)],
        Color32::from_rgb(200, 200, 204),
        Stroke::NONE,
    ));
    resp
        .on_hover_cursor(CursorIcon::PointingHand)
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
            match web::Browser::new(frame, &self.start_url, bounds, self.title.clone(), self.new_tx.clone(), self.incognito, self.fs.clone(), self.pip.clone()) {
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

struct App {
    tabs: Vec<Pane>,
    active: usize,
    addr: String,
    addr_focused: bool,
    tools_open: bool,
    /// Какие окна функций открыты (в порядке TOOLS).
    tool_win: [bool; 8],
    dots_open: bool,
    /// Сейчас окно на весь экран из-за страницы; адрес, с которого это началось.
    page_fs: bool,
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
        let (urls, active) = if bcfg.restore { store::load_session() } else { (Vec::new(), 0) };
        let (tabs, active) = if urls.is_empty() {
            (vec![Pane::new(&normalize_url(&bcfg.home))], 0)
        } else {
            (urls.iter().map(|u| Pane::new(u)).collect(), active)
        };
        let mut app = App {
            tabs,
            active,
            addr: String::new(),
            addr_focused: false,
            tools_open: false,
            tool_win: [false; 8],
            dots_open: false,
            page_fs: false,
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
            self.addr = self.tabs[i].current_url();
            self.addr_focused = false;
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        if self.tabs.is_empty() {
            self.tabs.push(Pane::new(&normalize_url(&self.bcfg.home)));
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
        self.addr = url;
    }

    // ── каркас окна ──

    fn top_bar(&mut self, ctx: &egui::Context) {
        let mut go: Option<String> = None;
        let mut toggle = false;
        let mut toggle_dots = false;
        let has = self.tabs[self.active].view.is_some();
        egui::TopBottomPanel::top("top")
            .frame(Frame::new().fill(SIDE_BG).inner_margin(Margin::symmetric(12, 8)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let tab = &self.tabs[self.active];
                    if icon_button(ui, NavIcon::Back, has).clicked() {
                        if let Some(b) = &tab.view {
                            b.back();
                        }
                    }
                    if icon_button(ui, NavIcon::Forward, has).clicked() {
                        if let Some(b) = &tab.view {
                            b.forward();
                        }
                    }
                    if icon_button(ui, NavIcon::Reload, has).clicked() {
                        if let Some(b) = &tab.view {
                            b.reload();
                        }
                    }
                    let width = (ui.available_width() - 86.0 - 56.0).max(120.0);
                    Frame::new()
                        .fill(FIELD_BG)
                        .corner_radius(CornerRadius::same(20))
                        .inner_margin(Margin::symmetric(14, 6))
                        .show(ui, |ui| {
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.addr)
                                    .frame(false)
                                    .desired_width(width - 28.0)
                                    .hint_text("Адрес сайта или запрос для поиска")
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
                    if sticker_button(ui, self.tools_open).clicked() {
                        toggle = true;
                    }
                    if dots_button(ui, self.dots_open).clicked() {
                        toggle_dots = true;
                    }
                });
            });
        if let Some(g) = go {
            self.go(&g);
        }
        if toggle {
            self.tools_open = !self.tools_open;
            self.dots_open = false;
        }
        if toggle_dots {
            self.dots_open = !self.dots_open;
            self.tools_open = false;
        }
    }

    fn tabs_panel(&mut self, ctx: &egui::Context) {
        let mut select: Option<usize> = None;
        let mut close: Option<usize> = None;
        let mut new_tab = false;
        let mut open_downloads = false;
        let active_downloads = self.downloads.iter().filter(|d| d.state == store::DL_ACTIVE).count();
        egui::SidePanel::left("tabs")
            .exact_width(220.0)
            .resizable(false)
            .frame(Frame::new().fill(SIDE_BG).inner_margin(Margin::same(10)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    paint_logo(ui.painter(), r);
                    ui.label(RichText::new("Petal").size(22.0).strong().color(TEXT));
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Вкладки").size(13.0).color(MUTED));
                ui.add_space(4.0);
                if accent_button(ui, "+ Новая вкладка").clicked() {
                    new_tab = true;
                }
                ui.add_space(8.0);
                egui::TopBottomPanel::bottom("tabs_bottom").frame(Frame::NONE).show_inside(ui, |ui| {
                    ui.add_space(6.0);
                    if downloads_button(ui, active_downloads).clicked() {
                        open_downloads = true;
                    }
                });
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (i, tab) in self.tabs.iter().enumerate() {
                        let active = i == self.active;
                        let (row, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
                        let close_rect = egui::Rect::from_center_size(row.right_center() - Vec2::new(16.0, 0.0), Vec2::splat(22.0));
                        let close_resp = ui.interact(close_rect, ui.id().with(("close_tab", i)), Sense::click());
                        let bg = if active {
                            Color32::from_rgb(48, 56, 78)
                        } else if resp.hovered() {
                            Color32::from_rgb(40, 41, 44)
                        } else {
                            Color32::TRANSPARENT
                        };
                        let p = ui.painter();
                        p.rect_filled(row, CornerRadius::same(10), bg);
                        let text_rect = egui::Rect::from_min_max(
                            row.left_top() + Vec2::new(12.0, 0.0),
                            egui::pos2(close_rect.left() - 2.0, row.bottom()),
                        );
                        p.with_clip_rect(text_rect).text(
                            row.left_center() + Vec2::new(12.0, 0.0),
                            Align2::LEFT_CENTER,
                            tab.label(),
                            FontId::proportional(14.0),
                            if active { LINK } else { TEXT },
                        );
                        if close_resp.hovered() {
                            p.circle_filled(close_rect.center(), 10.0, Color32::from_rgb(70, 72, 78));
                        }
                        p.text(
                            close_rect.center(),
                            Align2::CENTER_CENTER,
                            "×",
                            FontId::proportional(17.0),
                            if close_resp.hovered() { TEXT } else { MUTED },
                        );
                        if close_resp.clicked() || resp.middle_clicked() {
                            close = Some(i);
                        } else if resp.clicked() {
                            select = Some(i);
                        }
                        resp.on_hover_cursor(CursorIcon::PointingHand);
                    }
                });
            });
        if open_downloads {
            let i = TOOLS.iter().position(|t| t.0 == Tool::Downloads).unwrap_or(0);
            self.tool_win[i] = true;
            let id = egui::ViewportId::from_hash_of(("tool", TOOLS[i].1));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        if new_tab {
            self.open_tab(&normalize_url(&self.bcfg.home.clone()));
        } else if let Some(i) = close {
            self.close_tab(i);
        } else if let Some(i) = select {
            self.select_tab(i);
        }
    }

    /// Маленькое меню по значку-стикеру: кнопка на каждую функцию.
    fn tools_menu(&mut self, ctx: &egui::Context) {
        let mut picked: Option<usize> = None;
        let mut open_google = false;
        egui::SidePanel::right("tools_menu")
            .exact_width(190.0 + 20.0)
            .resizable(false)
            .frame(Frame::new().fill(CARD_BG).inner_margin(Margin::same(10)))
            .show(ctx, |ui| {
                ui.set_width(190.0);
                ui.label(RichText::new("Функции").size(13.0).color(MUTED));
                ui.add_space(4.0);
                for (i, (tool, name, color)) in TOOLS.iter().enumerate() {
                    if in_dots(*tool) || *tool == Tool::Downloads {
                        continue;
                    }
                    let text = RichText::new(name.to_string()).size(15.0).color(if self.tool_win[i] { *color } else { TEXT });
                    if ui
                        .add_sized([ui.available_width(), 34.0], egui::Button::new(text))
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        picked = Some(i);
                    }
                }
                ui.add_space(2.0);
                if ui
                    .add_sized([ui.available_width(), 34.0], egui::Button::new(RichText::new("Google").size(15.0).color(TEXT)))
                    .clicked()
                {
                    open_google = true;
                }
            });
        if open_google {
            self.go(HOME_URL);
            self.tools_open = false;
        }
        if let Some(i) = picked {
            self.tool_win[i] = true;
            self.tools_open = false;
            let id = egui::ViewportId::from_hash_of(("tool", TOOLS[i].1));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
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
                Tool::Settings => (560.0, 520.0),
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
    fn dots_menu(&mut self, ctx: &egui::Context) {
        let mut picked: Option<usize> = None;
        let mut incognito = false;
        egui::SidePanel::right("dots_menu")
            .exact_width(230.0 + 20.0)
            .resizable(false)
            .frame(Frame::new().fill(CARD_BG).inner_margin(Margin::same(10)))
            .show(ctx, |ui| {
                ui.set_width(230.0);
                if menu_item(ui, "Новая вкладка инкогнито", PURPLE) {
                    incognito = true;
                }
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                for (i, (tool, name, color)) in TOOLS.iter().enumerate() {
                    if in_dots(*tool) && menu_item(ui, name, if self.tool_win[i] { *color } else { TEXT }) {
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
        self.tabs.push(Pane::private(&normalize_url(&self.bcfg.home)));
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
                ui.label(RichText::new(d.name()).size(16.0).color(LINK));
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
            clip.text(rect.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, title, FontId::proportional(15.0), LINK);
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

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        header(ui, "Настройки", "Браузер Petal");
        card(ui, |ui| {
            ui.checkbox(&mut self.bcfg.restore, "Открывать при запуске вкладки, которые были при закрытии");
            ui.add_space(4.0);
            ui.checkbox(&mut self.bcfg.save_history, "Записывать историю посещений");
            ui.add_space(10.0);
            ui.label(RichText::new("Страница новой вкладки").color(MUTED));
            ui.add(egui::TextEdit::singleline(&mut self.bcfg.home).desired_width(f32::INFINITY));
        });
        ui.add_space(10.0);
        card(ui, |ui| {
            ui.label(RichText::new("Данные").size(17.0).color(TEXT));
            ui.add_space(6.0);
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
            });
            if !self.settings_status.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&self.settings_status).color(GREEN));
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "Petal {}. Данные лежат в {}. Вкладки инкогнито ни в историю, ни в сессию не попадают.",
                    env!("CARGO_PKG_VERSION"),
                    std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default()
                ))
                    .size(13.0)
                    .color(MUTED),
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
                ui.label(RichText::new(&e.site).size(16.0).color(LINK));
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
            ui.label(RichText::new("Звук в микрофон").size(13.0).color(LINK));
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
                ui.label(RichText::new("Новое название звука").size(13.0).color(LINK));
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
                    painter.rect_filled(done, CornerRadius::same(2), LINK);
                    // Значок «играет»: маленький треугольник.
                    let o = egui::pos2(rect.right() - 20.0, rect.top() + 10.0);
                    painter.add(egui::Shape::convex_polygon(
                        vec![o, o + Vec2::new(0.0, 12.0), o + Vec2::new(10.0, 6.0)],
                        LINK,
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
            ui.horizontal(|ui| {
                if accent_button(ui, "Бросить кубик").clicked() {
                    self.rnd_result = format!("Кубик: {}", self.rng.range(1, 6));
                }
                if accent_button(ui, "Подбросить монетку").clicked() {
                    self.rnd_result =
                        if self.rng.below(2) == 0 { "Монетка: орёл".into() } else { "Монетка: решка".into() };
                }
            });
            ui.horizontal(|ui| {
                ui.label("От");
                ui.add(egui::DragValue::new(&mut self.rnd_min).range(-1_000_000_000_000i64..=1_000_000_000_000i64));
                ui.label("до");
                ui.add(egui::DragValue::new(&mut self.rnd_max).range(-1_000_000_000_000i64..=1_000_000_000_000i64));
                if pill_button(ui, "Случайное число").clicked() {
                    let n = self.rng.range(self.rnd_min, self.rnd_max);
                    self.rnd_result = format!("Число: {n}");
                }
            });
            if !self.rnd_result.is_empty() {
                ui.label(RichText::new(&self.rnd_result).size(34.0).color(TEXT));
            }
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
                    ui.label(RichText::new("Варианты, по одному в строке").size(13.0).color(LINK));
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
                ui.label(RichText::new("Выпало").size(13.0).color(LINK));
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
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.calc_input)
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(24.0))
                    .hint_text("(2+3)*4^2"),
            );
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
                        let btn = egui::Button::new(RichText::new(*k).size(17.0)).min_size(Vec2::new(key_w, 44.0));
                        let btn = if *k == "=" { btn.fill(Color32::from_rgb(48, 80, 140)) } else { btn };
                        if ui.add(btn).on_hover_cursor(CursorIcon::PointingHand).clicked() {
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
                    ui.label(RichText::new("История").size(13.0).color(LINK));
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
            // Верхняя полоса идёт первой, чтобы тянуться на всю ширину окна.
            self.top_bar(ctx);
            self.tabs_panel(ctx);
            if self.tools_open {
                self.tools_menu(ctx);
            }
            if self.dots_open {
                self.dots_menu(ctx);
            }
        }
        self.tool_windows(ctx);

        let active = self.active;
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if i != active {
                t.park();
            }
        }
        egui::CentralPanel::default().frame(Frame::new().fill(BG)).show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            ui.allocate_rect(rect, Sense::hover());
            self.tabs[active].show(frame, ui, rect);
        });

        // Адрес и названия вкладок меняются сами при переходах.
        if !self.addr_focused {
            if let Some(u) = self.tabs[active].url() {
                if !u.is_empty() && u != self.addr {
                    self.addr = u;
                }
            }
        }
        if self.tabs.iter().any(|t| t.view.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        self.persist_tick();
        if self.cfg != self.saved_cfg {
            save_settings(&self.cfg);
            self.saved_cfg = self.cfg.clone();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
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
