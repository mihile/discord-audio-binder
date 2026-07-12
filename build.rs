fn main() {
    #[cfg(target_os = "windows")]
    winres::WindowsResource::new()
        .set_icon("assets/app-icon.ico")
        .compile()
        .expect("failed to embed application icon");
}
