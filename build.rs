fn main() {
    // Icon id 1: Explorer shows the exe with it and the tray loads it by that id.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("purrch.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
}
