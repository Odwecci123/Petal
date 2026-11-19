// Встраивает в petal.exe значок и сведения о версии (видны в проводнике и диспетчере задач).
fn main() {
    println!("cargo:rerun-if-changed=assets/petal.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/petal.ico");
        res.set("ProductName", "Petal");
        res.set("FileDescription", "Petal");
        if let Err(e) = res.compile() {
            // Без значка программа всё равно работает: не ломаем из-за этого сборку.
            println!("cargo:warning=не удалось встроить значок: {e}");
        }
    }
}
